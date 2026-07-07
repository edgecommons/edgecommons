"""
Standalone messaging provider for dual broker MQTT connections.

This provider supports connecting to both local and northbound brokers
simultaneously in STANDALONE mode, similar to the Java version.

The two transports (local broker and northbound broker) are identical apart from their
connection details and per-call QoS, so each broker's state lives in a
``_BrokerChannel`` and every operation is implemented once against a channel.
The public ``*_northbound`` methods are thin wrappers that
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

from edgecommons.messaging.message import Message
from edgecommons.messaging.messaging_provider import MessagingProvider, DEFAULT_MAX_MESSAGES
from edgecommons.messaging.qos import Qos
from edgecommons.messaging.messaging_config import MessagingConfiguration, QosDefaults
from edgecommons.utils.iou import Iou

logger = logging.getLogger(__name__)

# Human-readable label per channel name, used in messages/logs.
_BROKER_LABEL = {"local": "Local", "northbound": "Northbound"}


class _BrokerChannel:
    """Per-broker connection state (one MQTT client plus its subscription
    bookkeeping). ``name`` is "local" or "northbound" and is also the key used by
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

    Connects to both local and northbound brokers simultaneously, routing
    messages appropriately based on destination.
    """

    def __init__(self, config: MessagingConfiguration, thing_name: str):
        super().__init__()
        self._config = config
        self._messaging_config = config.messaging
        self._thing_name = thing_name
        self._local = _BrokerChannel("local")
        self._northbound = _BrokerChannel("northbound")
        # Pending request/reply futures, keyed by (unique) reply topic. Shared
        # across both channels — reply topics are unique, so a reply arriving on
        # either broker resolves the right Iou.
        self._response_ious: Dict[str, Iou] = {}
        self._executor = ThreadPoolExecutor(max_workers=10)
        self._lock = threading.RLock()
        self._subscription_timeout = 5.0

        self._initialize_clients()

    @property
    def _local_publish_qos(self) -> int:
        qos = self._messaging_config.local.qos if self._messaging_config.local else QosDefaults()
        return qos.publish

    @property
    def _local_subscribe_qos(self) -> int:
        qos = self._messaging_config.local.qos if self._messaging_config.local else QosDefaults()
        return qos.subscribe

    @property
    def _northbound_publish_qos(self) -> int:
        qos = self._messaging_config.northbound.qos if self._messaging_config.northbound else QosDefaults()
        return qos.publish

    @property
    def _northbound_subscribe_qos(self) -> int:
        qos = self._messaging_config.northbound.qos if self._messaging_config.northbound else QosDefaults()
        return qos.subscribe

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

            if self._messaging_config.northbound:
                cfg = self._messaging_config.northbound
                logger.info(f"Configuring northbound broker connection to {cfg.endpoint}:{cfg.port}")
                self._northbound.client = self._create_mqtt_client(cfg, self._northbound)
                self._connect_client(self._northbound, cfg)
            else:
                logger.info("Northbound broker configuration not provided, skipping northbound connection")

            logger.info("STANDALONE mode dual broker initialization completed successfully")

        except Exception as e:
            logger.error(f"Failed to initialize MQTT clients in STANDALONE mode: {e}")
            raise

    @staticmethod
    def _mqtt_qos(qos: Qos) -> int:
        """Map an EdgeCommons QoS to a paho MQTT QoS level (0, 1, or 2)."""
        return qos.mqtt_level

    def _create_mqtt_client(self, broker_config, channel: _BrokerChannel) -> mqtt.Client:
        """Create and configure an MQTT client for ``channel``."""
        broker_type = channel.name
        client_id = self._generate_client_id(broker_config, broker_type)
        logger.debug(f"Creating {broker_type} MQTT client with ID: {client_id}")

        client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id=client_id)

        # Configure TLS when a CA is configured — server-only or, with a client cert,
        # mutual TLS. This applies equally to local and northbound brokers.
        if broker_type in ("local", "northbound") and getattr(broker_config, 'credentials', None):
            creds = broker_config.credentials
            if getattr(creds, "ca_path", None):
                self._configure_tls(client, broker_config, broker_type)

        # Configure username/password authentication for either broker.
        if broker_type in ("local", "northbound") and getattr(broker_config, 'credentials', None):
            creds = broker_config.credentials
            username = getattr(creds, "username", None)
            password = getattr(creds, "password", None)
            if username and password:
                client.username_pw_set(username, password)

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
        client_id = self._thing_name or "edgecommons"
        logger.debug(f"Using default {broker_type} client ID: {client_id}")
        return client_id

    def _configure_tls(self, client: mqtt.Client, broker_config, broker_type: str):
        """Configure TLS for an MQTT client.

        TLS is keyed on caPath for both local and northbound brokers: with a CA
        only we do server-only TLS (verify the broker's certificate); if a client
        cert+key are also present we additionally present them for mutual TLS.
        """
        creds = getattr(broker_config, 'credentials', None)
        ca = getattr(creds, 'ca_path', None) if creds else None
        cert = getattr(creds, 'cert_path', None) if creds else None
        key = getattr(creds, 'key_path', None) if creds else None

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
            host = getattr(broker_config, "host", None) or getattr(broker_config, "endpoint", None)
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
    def _run_message(permits, semaphore, callback, topic, msg):
        """Run a subscription callback, honoring the maxConcurrency cap (semaphore) and releasing
        the per-subscription queue permit (max_messages bound) when done."""
        try:
            if semaphore is not None:
                semaphore.acquire()
                try:
                    callback(topic, msg)
                finally:
                    semaphore.release()
            else:
                callback(topic, msg)
        finally:
            if permits is not None:
                permits.release()

    def _process_message(self, message: mqtt.MQTTMessage, channel: _BrokerChannel):
        """Process a received MQTT message for a channel."""
        topic = message.topic
        logger.debug(f"Processing message from {channel.name} broker - topic: {topic}, "
                     f"size: {len(message.payload)} bytes, QoS: {message.qos}")

        try:
            try:
                msg = Message.from_bytes(message.payload)
            except ValueError as error:
                logger.warning(
                    "Problem decoding MQTT payload into EdgeCommons protobuf Message on topic "
                    "%s from %s broker: %s. Ignoring message.",
                    topic,
                    channel.name,
                    error,
                )
                return

            # Resolve a pending request/reply first. Reply arrival races the single
            # idempotent settle path (UNS-CANONICAL-DESIGN §5.1) against the framework
            # deadline and cancel_request: the winner owns the cleanup (unsubscribe on
            # the OWNING channel + pending-entry removal) and completes the future; a
            # loser (straggler reply after settle) is dropped at DEBUG.
            with self._lock:
                pending = self._response_ious.get(topic)
            if pending is not None:
                if pending.try_settle():
                    logger.debug(f"Message from {channel.name} broker matches pending request on {topic}")
                    # Tear down the one-shot reply subscription so it does not leak on the broker
                    # (mirrors the IPC path and _cancel_request); otherwise every timed-out-or-served
                    # request orphans a subscription and eventually trips the broker's sub quota.
                    self._unsubscribe(channel, topic)
                    with self._lock:
                        self._response_ious.pop(topic, None)
                    pending.set_result(msg)
                else:
                    logger.debug(f"Dropping straggler reply on '{topic}' (request already settled)")
                return

            # Otherwise dispatch to the first matching subscription.
            for topic_filter, sub_info in channel.subscriptions.items():
                if self.topic_matches_sub(topic_filter, topic):
                    callback = sub_info['callback']
                    if callback:
                        # Per-subscription queue bound (max_messages): drop on overflow rather than
                        # letting the shared executor's backlog grow unbounded (parity with Rust/TS).
                        permits = sub_info.get('queue_permits')
                        if permits is not None and not permits.acquire(blocking=False):
                            logger.warning(
                                f"subscription queue full (max_messages={sub_info.get('max_messages')}) "
                                f"for filter '{topic_filter}'; dropping message on {topic}"
                            )
                        else:
                            logger.debug(f"Dispatching {channel.name} message on {topic} (filter: {topic_filter})")
                            self._executor.submit(
                                self._run_message, permits, sub_info.get('semaphore'), callback, topic, msg
                            )
                    return

            logger.debug(f"No subscription found for {channel.name} topic: {topic}")

        except Exception as e:
            logger.error(f"Error processing message from {channel.name} broker on topic {topic}: {e}",
                         exc_info=True)
            # Don't re-raise - this could cause disconnection.

    def connected(self) -> bool:
        """Report the broker connection state for readiness (FR-HB-1).

        Tracks the **local** broker ONLY — it carries in-cluster pub/sub and is the connection
        readiness should gate on; a northbound drop alone must not flip readiness while the local
        broker serves. There is deliberately NO northbound fallback, matching the canonical Java
        ``StandaloneMessagingProvider.connected()`` (local-only) and the Rust/TS providers. Returns
        ``False`` when the local client is absent or down. Backed by paho's ``client.is_connected()``.
        """
        client = self._local.client
        if client is None:
            return False
        try:
            return bool(client.is_connected())
        except Exception:  # noqa: BLE001 - readiness check must never raise
            return False

    def disconnect(self):
        """Disconnect from all brokers and release resources."""
        logger.info("Initiating STANDALONE mode broker disconnection")

        for channel in (self._local, self._northbound):
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
            payload = msg.to_bytes()
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
                   mqtt_qos: int, max_concurrency, max_messages=None):
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
            effective_max = max_messages if max_messages is not None else DEFAULT_MAX_MESSAGES
            channel.subscriptions[topic] = {
                'callback': callback,
                'max_concurrency': max_concurrency,
                'semaphore': self._make_semaphore(max_concurrency),
                'max_messages': effective_max,
                # Bounded permit (drop on overflow) when > 0, else None (unbounded).
                'queue_permits': threading.Semaphore(effective_max) if effective_max and effective_max > 0 else None,
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
                 reply_qos: int, publish_qos: int,
                 timeout_secs: Optional[float] = None) -> Iou:
        reply_topic = f"edgecommons/reply-{uuid.uuid4()}"
        # Carry the reply topic as the Iou's user_data so cancel_request() can
        # find and tear down the right subscription/pending entry.
        iou = Iou(reply_topic)
        with self._lock:
            self._response_ious[reply_topic] = iou

        msg.get_header().reply_to = reply_topic
        self._subscribe(channel, reply_topic, None, reply_qos, None)

        # Arm the framework-owned deadline at send time (UNS-CANONICAL-DESIGN §5): on
        # expiry the timer unsubscribes the ephemeral reply topic (on the owning
        # channel), removes the pending entry and completes the future exceptionally
        # (RequestTimeoutError) — even when the caller never get()'s the future.
        def _deadline_cleanup():
            with self._lock:
                self._response_ious.pop(reply_topic, None)
            self._unsubscribe(channel, reply_topic)

        self._arm_request_deadline(iou, self._effective_request_timeout(timeout_secs),
                                   _deadline_cleanup)

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
        if not iou.try_settle():
            return  # reply or deadline already settled + cleaned up this request
        topic = iou.get_user_data()
        with self._lock:
            self._response_ious.pop(topic, None)
        self._unsubscribe(channel, topic)
        iou.set_result(None)

    # ----- public messaging API (local transport) -------------------------------------

    def publish(self, topic: str, msg: Message):
        """Publish message to local broker."""
        self._publish(self._local, topic, msg, self._local_publish_qos)

    def subscribe(self, topic: str, callback: Callable[[str, Message], None], max_concurrency: int = None,
                  max_messages: int = None):
        """Subscribe to topic on local broker and wait for confirmation."""
        self._subscribe(self._local, topic, callback, self._local_subscribe_qos, max_concurrency, max_messages)

    def request(self, topic: str, msg: Message, timeout_secs: Optional[float] = None) -> Iou:
        """Send request to local broker and wait for response.

        ``timeout_secs``: the per-call deadline (UNS-CANONICAL-DESIGN §5) — ``None``
        uses the configured default; ``0`` disables the deadline for this call.
        """
        return self._request(self._local, topic, msg, reply_qos=self._local_subscribe_qos, publish_qos=self._local_publish_qos,
                             timeout_secs=timeout_secs)

    def reply(self, request: Message, reply: Message):
        """Send reply to local broker."""
        self._reply(self._local, request, reply, publish_qos=self._local_publish_qos)

    def publish_raw(self, topic: str, msg: dict):
        """Publish raw message to local broker."""
        self._publish_raw(self._local, topic, msg, mqtt_qos=self._local_publish_qos)

    def unsubscribe(self, topic: str):
        """Unsubscribe from topic on local broker."""
        self._unsubscribe(self._local, topic)

    def cancel_request(self, iou: Iou):
        """Cancel pending request to local broker."""
        self._cancel_request(self._local, iou)

    # ----- public messaging API (northbound transport) --------------------------------

    def publish_northbound(self, topic: str, msg: Message, qos: Qos):
        """Publish message to the northbound broker."""
        self._publish(self._northbound, topic, msg, self._mqtt_qos(qos))

    def subscribe_northbound(self, topic: str, callback: Callable[[str, Message], None],
                              qos: Qos, max_concurrency: int = None, max_messages: int = None):
        """Subscribe to topic on the northbound broker and wait for confirmation."""
        self._subscribe(self._northbound, topic, callback, self._mqtt_qos(qos), max_concurrency, max_messages)

    def request_northbound(self, topic: str, msg: Message,
                              timeout_secs: Optional[float] = None) -> Iou:
        """Send request to the northbound broker and wait for response (same deadline
        semantics as :meth:`request`)."""
        # Standalone northbound MQTT defaults come from messaging.northbound.qos.
        return self._request(self._northbound, topic, msg, reply_qos=self._northbound_subscribe_qos,
                             publish_qos=self._northbound_publish_qos,
                             timeout_secs=timeout_secs)

    def reply_northbound(self, request: Message, reply: Message):
        """Send reply to the northbound broker."""
        self._reply(self._northbound, request, reply, publish_qos=self._northbound_publish_qos)

    def publish_northbound_raw(self, topic: str, msg: dict, qos: Qos):
        """Publish raw message to the northbound broker."""
        self._publish_raw(self._northbound, topic, msg, self._mqtt_qos(qos))

    def unsubscribe_northbound(self, topic: str):
        """Unsubscribe from topic on the northbound broker."""
        self._unsubscribe(self._northbound, topic)

    def cancel_request_northbound(self, iou: Iou):
        """Cancel pending request to the northbound broker."""
        self._cancel_request(self._northbound, iou)

    # ----- misc ------------------------------------------------------------------------

    def get_native_client(self):
        """Get native MQTT clients."""
        return {'local': self._local.client, 'northbound': self._northbound.client}
