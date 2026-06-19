"""
Standalone messaging provider for dual broker MQTT connections.

This provider supports connecting to both local and IoT Core brokers
simultaneously in STANDALONE mode, similar to the Java version.

The two transports (local broker and IoT Core) are identical apart from their
connection details and per-call QoS, so each broker's state lives in a
``_BrokerChannel`` and every operation is implemented once against a channel.
The public ``*_to_iot_core`` / ``*_from_iot_core`` methods are thin wrappers that
select the channel and the right QoS.
"""

import json
import logging
import ssl
import threading
import time
import uuid
from typing import Callable, Dict, Optional
from concurrent.futures import ThreadPoolExecutor

import paho.mqtt.client as mqtt
from awsiot.greengrasscoreipc.model import QOS

from ggcommons.messaging.message import Message
from ggcommons.messaging.messaging_provider import MessagingProvider
from ggcommons.messaging.messaging_config import MessagingConfiguration
from ggcommons.utils.iou import Iou

logger = logging.getLogger(__name__)

# Human-readable label per channel name, used in messages/logs.
_BROKER_LABEL = {"local": "Local", "iotcore": "IoT Core"}


class _BrokerChannel:
    """Per-broker connection state (one MQTT client plus its subscription
    bookkeeping). ``name`` is "local" or "iotcore" and is also the key used by
    the TLS/auth/connect logic and log messages."""

    def __init__(self, name: str):
        self.name = name
        self.client: Optional[mqtt.Client] = None
        self.subscriptions: Dict[str, dict] = {}
        self.pending_subscriptions: Dict[str, threading.Event] = {}
        self.mid_to_topic: Dict[int, str] = {}


class StandaloneProvider(MessagingProvider):
    """
    Standalone messaging provider supporting dual broker connections.

    Connects to both local and IoT Core brokers simultaneously, routing
    messages appropriately based on destination.
    """

    def __init__(self, config: MessagingConfiguration, thing_name: str):
        super().__init__()
        self._config = config
        self._messaging_config = config.messaging
        self._thing_name = thing_name
        self._local = _BrokerChannel("local")
        self._iot_core = _BrokerChannel("iotcore")
        # Pending request/reply futures, keyed by (unique) reply topic. Shared
        # across both channels — reply topics are unique, so a reply arriving on
        # either broker resolves the right Iou.
        self._response_ious: Dict[str, Iou] = {}
        self._executor = ThreadPoolExecutor(max_workers=10)
        self._lock = threading.RLock()
        self._subscription_timeout = 5.0

        self._initialize_clients()

    def _initialize_clients(self):
        """Initialize MQTT clients for the configured brokers."""
        logger.info("Initializing STANDALONE mode dual broker connections")

        try:
            if self._messaging_config.local:
                cfg = self._messaging_config.local
                logger.info(f"Configuring local broker connection to {cfg.host}:{cfg.port}")
                self._local.client = self._create_mqtt_client(cfg, self._local)
                self._connect_client(self._local, cfg)
            else:
                logger.info("Local broker configuration not provided, skipping local connection")

            if self._messaging_config.iot_core:
                cfg = self._messaging_config.iot_core
                logger.info(f"Configuring IoT Core broker connection to {cfg.endpoint}:{cfg.port}")
                self._iot_core.client = self._create_mqtt_client(cfg, self._iot_core)
                self._connect_client(self._iot_core, cfg)
            else:
                logger.info("IoT Core broker configuration not provided, skipping IoT Core connection")

            logger.info("STANDALONE mode dual broker initialization completed successfully")

        except Exception as e:
            logger.error(f"Failed to initialize MQTT clients in STANDALONE mode: {e}")
            raise

    @staticmethod
    def _mqtt_qos(qos: QOS) -> int:
        """Map a Greengrass QOS to a paho MQTT QoS level (0 or 1)."""
        return 0 if qos == QOS.AT_MOST_ONCE else 1

    def _create_mqtt_client(self, broker_config, channel: _BrokerChannel) -> mqtt.Client:
        """Create and configure an MQTT client for ``channel``."""
        broker_type = channel.name
        client_id = self._generate_client_id(broker_config, broker_type)
        logger.debug(f"Creating {broker_type} MQTT client with ID: {client_id}")

        client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id=client_id)

        # Configure TLS for IoT Core (required) or the local broker (when a CA is
        # configured — server-only or, with a client cert, mutual TLS).
        if broker_type == "iotcore":
            self._configure_tls(client, broker_config, "iotcore")
        elif broker_type == "local" and getattr(broker_config, 'credentials', None):
            creds = broker_config.credentials
            if creds.ca_path:
                self._configure_tls(client, broker_config, "local")

        # Configure username/password authentication for the local broker.
        if broker_type == "local" and getattr(broker_config, 'credentials', None):
            creds = broker_config.credentials
            if creds.username and creds.password:
                client.username_pw_set(creds.username, creds.password)

        # Wire callbacks to this channel.
        client.on_message = lambda c, u, m: self._process_message(m, channel)
        client.on_connect = lambda c, u, f, rc, p: self._on_connect(channel, rc)
        client.on_disconnect = lambda c, u, f, rc, p: self._on_disconnect(channel, rc)
        client.on_subscribe = lambda c, u, mid, granted_qos, p: self._on_subscribe(channel, mid, granted_qos)

        logger.debug(f"Successfully created and configured {broker_type} MQTT client")
        return client

    def _generate_client_id(self, broker_config, broker_type: str) -> str:
        """Generate client ID for MQTT connection."""
        if getattr(broker_config, 'client_id', None):
            return broker_config.client_id
        client_id = self._thing_name or "ggcommons"
        logger.debug(f"Using default {broker_type} client ID: {client_id}")
        return client_id

    def _configure_tls(self, client: mqtt.Client, broker_config, broker_type: str):
        """Configure TLS for an MQTT client.

        IoT Core requires mutual TLS: if any of caPath/certPath/keyPath is missing
        we refuse to connect rather than silently falling back to an unauthenticated
        plaintext connection (C3). For the local broker, TLS is keyed on caPath
        alone (parity with Java/Rust): with a CA only we do server-only TLS
        (verify the broker's certificate); if a client cert+key are also present we
        additionally present them for mutual TLS.
        """
        creds = getattr(broker_config, 'credentials', None)
        ca = getattr(creds, 'ca_path', None) if creds else None
        cert = getattr(creds, 'cert_path', None) if creds else None
        key = getattr(creds, 'key_path', None) if creds else None

        if broker_type == "iotcore":
            if not (ca and cert and key):
                raise RuntimeError(
                    "Refusing to connect to IoT Core without complete TLS credentials "
                    "(caPath, certPath and keyPath are all required)"
                )
            ssl_context = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
            ssl_context.load_verify_locations(ca)
            ssl_context.load_cert_chain(cert, key)
            ssl_context.check_hostname = False
            ssl_context.verify_mode = ssl.CERT_REQUIRED
            client.tls_set_context(ssl_context)
            return

        # Local broker: TLS only when a CA is configured; client cert/key optional.
        if not ca:
            return
        ssl_context = ssl.create_default_context(ssl.Purpose.SERVER_AUTH)
        ssl_context.load_verify_locations(ca)
        if cert and key:
            ssl_context.load_cert_chain(cert, key)  # mutual TLS
        ssl_context.check_hostname = False
        ssl_context.verify_mode = ssl.CERT_REQUIRED
        client.tls_set_context(ssl_context)

    def _connect_client(self, channel: _BrokerChannel, broker_config):
        """Connect a channel's MQTT client to its broker and block until connected."""
        client = channel.client
        try:
            if channel.name == "iotcore":
                host = broker_config.endpoint
            else:
                host = broker_config.host
            client.connect_async(host, broker_config.port, 60)
            client.loop_start()

            # Block until connected or timeout.
            timeout = 5.0
            start_time = time.time()
            while not client.is_connected():
                if time.time() - start_time > timeout:
                    raise TimeoutError(
                        f"Failed to connect to {channel.name} broker at "
                        f"{host}:{broker_config.port} within {timeout} seconds"
                    )
                time.sleep(0.1)

            logger.info(f"Successfully connected to {channel.name} broker at {host}:{broker_config.port}")

        except Exception as e:
            logger.error(f"Failed to connect to {channel.name} broker: {e}")
            raise

    # ----- connection callbacks (channel-parameterized) --------------------------------

    def _on_connect(self, channel: _BrokerChannel, rc):
        """Handle MQTT connection for a channel."""
        if rc == 0:
            logger.info(f"Successfully connected to {channel.name} broker")
            # Re-establish subscriptions after a (re)connect. paho's loop does not
            # restore them automatically, so on a reconnect they would otherwise be
            # silently lost (M11). No-op on the first connect (nothing tracked yet).
            self._resubscribe(channel)
        else:
            logger.error(f"Failed to connect to {channel.name} broker with code {rc}")

    def _resubscribe(self, channel: _BrokerChannel) -> None:
        with self._lock:
            items = list(channel.subscriptions.items())
        if not items:
            return
        logger.info(f"Re-subscribing to {len(items)} {channel.name} topic(s) after connect")
        for topic, info in items:
            try:
                channel.client.subscribe(topic, qos=info.get("qos", 0))
            except Exception as e:
                logger.error(f"Failed to re-subscribe to {channel.name} topic {topic}: {e}")

    def _on_disconnect(self, channel: _BrokerChannel, rc):
        """Handle MQTT disconnection for a channel."""
        if rc == 0:
            logger.info(f"Clean disconnection from {channel.name} broker")
        else:
            logger.error(f"Unexpected disconnection from {channel.name} broker - code: {rc}")

        # Unblock and clear any in-flight subscription waits.
        with self._lock:
            for event in channel.pending_subscriptions.values():
                event.set()
            channel.pending_subscriptions.clear()
            channel.mid_to_topic.clear()

    def _on_subscribe(self, channel: _BrokerChannel, mid, granted_qos):
        """Handle a SUBACK for a channel: unblock the waiting subscribe()."""
        with self._lock:
            topic = channel.mid_to_topic.pop(mid, None)
            if topic and topic in channel.pending_subscriptions:
                event = channel.pending_subscriptions.pop(topic)
                if 0x80 in granted_qos:  # Subscription failed
                    logger.error(f"{channel.name} broker subscription failed for topic: {topic}")
                else:
                    logger.debug(f"{channel.name} broker subscription confirmed for topic: {topic}")
                event.set()

    # ----- inbound message dispatch ----------------------------------------------------

    @staticmethod
    def _make_semaphore(max_concurrency):
        """A bounded Semaphore for a positive maxConcurrency, else None (uncapped)."""
        if max_concurrency and max_concurrency > 0:
            return threading.Semaphore(max_concurrency)
        return None

    @staticmethod
    def _run_capped_callback(semaphore, callback, topic, msg):
        """Run a subscription callback while holding a concurrency permit."""
        semaphore.acquire()
        try:
            callback(topic, msg)
        finally:
            semaphore.release()

    def _process_message(self, message: mqtt.MQTTMessage, channel: _BrokerChannel):
        """Process a received MQTT message for a channel."""
        topic = message.topic
        logger.debug(f"Processing message from {channel.name} broker - topic: {topic}, "
                     f"size: {len(message.payload)} bytes, QoS: {message.qos}")

        try:
            payload = message.payload.decode('utf-8')
            try:
                # Use Message.from_object (same as the IPC path): a non-envelope
                # JSON payload becomes a raw message (.raw), matching Java/Rust.
                msg = Message.from_object(json.loads(payload))
            except json.JSONDecodeError:
                logger.debug(f"Message from {channel.name} broker is not JSON, treating as raw payload")
                msg = Message()
                msg.raw = payload

            # Resolve a pending request/reply first.
            with self._lock:
                if topic in self._response_ious:
                    logger.debug(f"Message from {channel.name} broker matches pending request on {topic}")
                    self._response_ious.pop(topic).set_result(msg)
                    return

            # Otherwise dispatch to the first matching subscription.
            for topic_filter, sub_info in channel.subscriptions.items():
                if self.topic_matches_sub(topic_filter, topic):
                    callback = sub_info['callback']
                    if callback:
                        logger.debug(f"Dispatching {channel.name} message on {topic} (filter: {topic_filter})")
                        semaphore = sub_info.get('semaphore')
                        if semaphore is not None:
                            # Enforce the per-subscription maxConcurrency cap.
                            self._executor.submit(
                                self._run_capped_callback, semaphore, callback, topic, msg
                            )
                        else:
                            self._executor.submit(callback, topic, msg)
                    return

            logger.debug(f"No subscription found for {channel.name} topic: {topic}")

        except Exception as e:
            logger.error(f"Error processing message from {channel.name} broker on topic {topic}: {e}",
                         exc_info=True)
            # Don't re-raise - this could cause disconnection.

    def disconnect(self):
        """Disconnect from all brokers and release resources."""
        logger.info("Initiating STANDALONE mode broker disconnection")

        for channel in (self._local, self._iot_core):
            if channel.client:
                channel.client.loop_stop()
                channel.client.disconnect()
                channel.client = None
                logger.info(f"Disconnected from {channel.name} broker")

        self._executor.shutdown(wait=True)
        logger.info("STANDALONE mode disconnection completed - all brokers disconnected and resources cleaned up")

    # ----- channel-parameterized operations --------------------------------------------

    def _require_client(self, channel: _BrokerChannel) -> mqtt.Client:
        if not channel.client:
            raise RuntimeError(f"{_BROKER_LABEL[channel.name]} broker client not available")
        return channel.client

    def _publish(self, channel: _BrokerChannel, topic: str, msg: Message, mqtt_qos: int):
        client = self._require_client(channel)
        try:
            payload = msg.dumps()
            logger.debug(f"Publishing to {channel.name} broker - topic: {topic}, "
                         f"size: {len(payload)} bytes, QoS: {mqtt_qos}")
            result = client.publish(topic, payload, qos=mqtt_qos)
            if result.rc != mqtt.MQTT_ERR_SUCCESS:
                logger.error(f"Failed to publish to {channel.name} broker topic {topic} - error code: {result.rc}")
        except Exception as e:
            logger.error(f"Error publishing message to {channel.name} broker topic {topic}: {e}")
            raise

    def _publish_raw(self, channel: _BrokerChannel, topic: str, msg: dict, mqtt_qos: int):
        client = self._require_client(channel)
        client.publish(topic, json.dumps(msg), qos=mqtt_qos)

    def _subscribe(self, channel: _BrokerChannel, topic: str,
                   callback: Optional[Callable[[str, Message], None]],
                   mqtt_qos: int, max_concurrency):
        client = self._require_client(channel)
        logger.debug(f"Subscribing to {channel.name} broker topic: {topic} (QoS: {mqtt_qos})")
        try:
            event = threading.Event()
            with self._lock:
                channel.pending_subscriptions[topic] = event

            result = client.subscribe(topic, qos=mqtt_qos)
            if result[0] != mqtt.MQTT_ERR_SUCCESS:
                with self._lock:
                    channel.pending_subscriptions.pop(topic, None)
                raise RuntimeError(f"Failed to send {channel.name} subscription request: {result[0]}")

            with self._lock:
                channel.mid_to_topic[result[1]] = topic

            # Block until SUBACK or timeout.
            if not event.wait(timeout=self._subscription_timeout):
                with self._lock:
                    channel.pending_subscriptions.pop(topic, None)
                    channel.mid_to_topic.pop(result[1], None)
                raise TimeoutError(
                    f"{channel.name} subscription to {topic} timed out after "
                    f"{self._subscription_timeout} seconds"
                )

            # Confirmed: store it (qos retained for re-subscribe on reconnect).
            channel.subscriptions[topic] = {
                'callback': callback,
                'max_concurrency': max_concurrency,
                'semaphore': self._make_semaphore(max_concurrency),
                'qos': mqtt_qos,
            }
            logger.debug(f"Successfully subscribed to {channel.name} broker topic: {topic}")

        except Exception as e:
            logger.error(f"Error subscribing to {channel.name} broker topic {topic}: {e}")
            raise

    def _unsubscribe(self, channel: _BrokerChannel, topic: str):
        if channel.client and topic in channel.subscriptions:
            channel.client.unsubscribe(topic)
            del channel.subscriptions[topic]

    def _request(self, channel: _BrokerChannel, topic: str, msg: Message,
                 reply_qos: int, publish_qos: int) -> Iou:
        reply_topic = f"ggcommons/reply-{uuid.uuid4()}"
        # Carry the reply topic as the Iou's user_data so cancel_request() can
        # find and tear down the right subscription/pending entry.
        iou = Iou(reply_topic)
        with self._lock:
            self._response_ious[reply_topic] = iou

        msg.get_header().reply_to = reply_topic
        self._subscribe(channel, reply_topic, None, reply_qos, None)
        self._publish(channel, topic, msg, publish_qos)
        logger.debug(f"Request sent to {channel.name} broker, awaiting response on {reply_topic}")
        return iou

    def _reply(self, channel: _BrokerChannel, request: Message, reply: Message, publish_qos: int):
        reply_topic = request.get_header().reply_to
        if not reply_topic:
            logger.error("Cannot send reply - no reply-to topic in request")
            raise ValueError("Request message missing reply-to topic")
        # Correlate the reply with the request so the requester can match it.
        reply.set_correlation_id(request.get_correlation_id())
        logger.debug(f"Sending reply to {channel.name} broker topic: {reply_topic}")
        self._publish(channel, reply_topic, reply, publish_qos)

    def _cancel_request(self, channel: _BrokerChannel, iou: Iou):
        topic = iou.get_user_data()
        with self._lock:
            self._response_ious.pop(topic, None)
        self._unsubscribe(channel, topic)

    # ----- public messaging API (local transport) -------------------------------------

    def publish(self, topic: str, msg: Message):
        """Publish message to local broker."""
        self._publish(self._local, topic, msg, 0)

    def subscribe(self, topic: str, callback: Callable[[str, Message], None], max_concurrency: int = None):
        """Subscribe to topic on local broker and wait for confirmation."""
        self._subscribe(self._local, topic, callback, 0, max_concurrency)

    def request(self, topic: str, msg: Message) -> Iou:
        """Send request to local broker and wait for response."""
        return self._request(self._local, topic, msg, reply_qos=0, publish_qos=0)

    def reply(self, request: Message, reply: Message):
        """Send reply to local broker."""
        self._reply(self._local, request, reply, publish_qos=0)

    def publish_raw(self, topic: str, msg: dict):
        """Publish raw message to local broker."""
        self._publish_raw(self._local, topic, msg, mqtt_qos=1)

    def unsubscribe(self, topic: str):
        """Unsubscribe from topic on local broker."""
        self._unsubscribe(self._local, topic)

    def cancel_request(self, iou: Iou):
        """Cancel pending request to local broker."""
        self._cancel_request(self._local, iou)

    # ----- public messaging API (IoT Core transport) ----------------------------------

    def publish_to_iot_core(self, topic: str, msg: Message, qos: QOS):
        """Publish message to IoT Core broker."""
        self._publish(self._iot_core, topic, msg, self._mqtt_qos(qos))

    def subscribe_to_iot_core(self, topic: str, callback: Callable[[str, Message], None],
                              qos: QOS, max_concurrency: int = None):
        """Subscribe to topic on IoT Core broker and wait for confirmation."""
        self._subscribe(self._iot_core, topic, callback, self._mqtt_qos(qos), max_concurrency)

    def request_from_iot_core(self, topic: str, msg: Message) -> Iou:
        """Send request to IoT Core broker and wait for response."""
        # Subscribe to the reply at QoS 0 (AT_MOST_ONCE); publish the request at
        # QoS 1 (AT_LEAST_ONCE) — matching the previous behavior.
        return self._request(self._iot_core, topic, msg, reply_qos=0, publish_qos=1)

    def reply_to_iot_core(self, request: Message, reply: Message):
        """Send reply to IoT Core broker."""
        self._reply(self._iot_core, request, reply, publish_qos=1)

    def publish_to_iot_core_raw(self, topic: str, msg: dict, qos: str):
        """Publish raw message to IoT Core broker."""
        self._publish_raw(self._iot_core, topic, msg, self._mqtt_qos(qos))

    def unsubscribe_from_iot_core(self, topic: str):
        """Unsubscribe from topic on IoT Core broker."""
        self._unsubscribe(self._iot_core, topic)

    def cancel_request_from_iot_core(self, iou: Iou):
        """Cancel pending request to IoT Core broker."""
        self._cancel_request(self._iot_core, iou)

    # ----- misc ------------------------------------------------------------------------

    def get_native_client(self):
        """Get native MQTT clients."""
        return {'local': self._local.client, 'iot_core': self._iot_core.client}
