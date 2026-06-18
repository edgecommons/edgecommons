"""
Standalone messaging provider for dual broker MQTT connections.

This provider supports connecting to both local and IoT Core brokers
simultaneously in STANDALONE mode, similar to the Java version.
"""

import json
import logging
import ssl
import threading
import uuid
from typing import Callable, Dict, Optional
from concurrent.futures import ThreadPoolExecutor

import paho.mqtt.client as mqtt
from awsiot.greengrasscoreipc.model import QOS

from ggcommons.messaging.message import Message
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.messaging.messaging_provider import MessagingProvider
from ggcommons.messaging.messaging_config import MessagingConfiguration
from ggcommons.utils.iou import Iou

logger = logging.getLogger(__name__)


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
        self._local_client: Optional[mqtt.Client] = None
        self._iot_core_client: Optional[mqtt.Client] = None
        self._subscriptions: Dict[str, dict] = {}
        self._iot_core_subscriptions: Dict[str, dict] = {}
        self._response_ious: Dict[str, Iou] = {}
        self._executor = ThreadPoolExecutor(max_workers=10)
        self._lock = threading.RLock()
        
        # Subscription state tracking
        self._pending_local_subscriptions: Dict[str, threading.Event] = {}
        self._pending_iot_core_subscriptions: Dict[str, threading.Event] = {}
        self._local_mid_to_topic: Dict[int, str] = {}
        self._iot_core_mid_to_topic: Dict[int, str] = {}
        self._subscription_timeout = 5.0
        
        self._initialize_clients()
    
    def _initialize_clients(self):
        """Initialize MQTT clients for local and IoT Core brokers."""
        logger.info("Initializing STANDALONE mode dual broker connections")
        
        try:
            # Initialize local broker client
            if self._messaging_config.local:
                logger.info(f"Configuring local broker connection to {self._messaging_config.local.host}:{self._messaging_config.local.port}")
                self._local_client = self._create_mqtt_client(
                    self._messaging_config.local, "local"
                )
                self._connect_client(self._local_client, self._messaging_config.local, "local")
                logger.debug(f"Local broker client initialized with ID: {self._local_client._client_id}")
            else:
                logger.info("Local broker configuration not provided, skipping local connection")
            
            # Initialize IoT Core broker client
            if self._messaging_config.iot_core:
                logger.info(f"Configuring IoT Core broker connection to {self._messaging_config.iot_core.endpoint}:{self._messaging_config.iot_core.port}")
                self._iot_core_client = self._create_mqtt_client(
                    self._messaging_config.iot_core, "iotcore"
                )
                self._connect_client(self._iot_core_client, self._messaging_config.iot_core, "iotcore")
                logger.debug(f"IoT Core broker client initialized with ID: {self._iot_core_client._client_id}")
            else:
                logger.info("IoT Core broker configuration not provided, skipping IoT Core connection")
                
            logger.info("STANDALONE mode dual broker initialization completed successfully")
                
        except Exception as e:
            logger.error(f"Failed to initialize MQTT clients in STANDALONE mode: {e}")
            raise
    
    def _create_mqtt_client(self, broker_config, broker_type: str) -> mqtt.Client:
        """Create and configure an MQTT client."""
        client_id = self._generate_client_id(broker_config, broker_type)
        logger.debug(f"Creating {broker_type} MQTT client with ID: {client_id}")
        
        client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id=client_id)
        
        # Configure TLS for IoT Core (required) or the local broker (when a CA is
        # configured — server-only or, with a client cert, mutual TLS).
        if broker_type == "iotcore":
            logger.debug(f"Configuring TLS for IoT Core broker connection")
            self._configure_tls(client, broker_config, "iotcore")
        elif broker_type == "local" and hasattr(broker_config, 'credentials') and broker_config.credentials:
            creds = broker_config.credentials
            if creds.ca_path:
                logger.debug(f"Configuring TLS for local broker connection (caPath present)")
                self._configure_tls(client, broker_config, "local")
            else:
                logger.debug(f"No caPath for local broker, using plain connection")
        
        # Configure authentication for local broker
        if broker_type == "local" and hasattr(broker_config, 'credentials') and broker_config.credentials:
            creds = broker_config.credentials
            if creds.username and creds.password:
                logger.debug(f"Configuring username/password authentication for local broker")
                client.username_pw_set(creds.username, creds.password)
            else:
                logger.debug(f"No username/password provided for local broker")
        
        # Set callbacks
        if broker_type == "local":
            client.on_message = self._on_local_message
            client.on_connect = lambda c, u, f, rc, p: self._on_connect(c, u, f, rc, p, "local")
            client.on_disconnect = lambda c, u, f, rc, p: self._on_disconnect(c, u, f, rc, p, "local")
            client.on_subscribe = self._on_local_subscribe
        else:
            client.on_message = self._on_iot_core_message
            client.on_connect = lambda c, u, f, rc, p: self._on_connect(c, u, f, rc, p, "iotcore")
            client.on_disconnect = lambda c, u, f, rc, p: self._on_disconnect(c, u, f, rc, p, "iotcore")
            client.on_subscribe = self._on_iot_core_subscribe
        
        logger.debug(f"Successfully created and configured {broker_type} MQTT client")
        return client
    
    def _generate_client_id(self, broker_config, broker_type: str) -> str:
        """Generate client ID for MQTT connection."""
        if hasattr(broker_config, 'client_id') and broker_config.client_id:
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
    
    def _connect_client(self, client: mqtt.Client, broker_config, broker_type: str):
        """Connect MQTT client to broker and wait for connection."""
        import time
        
        try:
            # Start async connection
            if broker_type == "iotcore":
                client.connect_async(broker_config.endpoint, broker_config.port, 60)
                host = broker_config.endpoint
            else:
                client.connect_async(broker_config.host, broker_config.port, 60)
                host = broker_config.host
            
            client.loop_start()
            
            # Block until connected or timeout
            timeout = 5.0
            start_time = time.time()
            while not client.is_connected():
                if time.time() - start_time > timeout:
                    raise TimeoutError(f"Failed to connect to {broker_type} broker at {host}:{broker_config.port} within {timeout} seconds")
                time.sleep(0.1)
            
            logger.info(f"Successfully connected to {broker_type} broker at {host}:{broker_config.port}")
            
        except Exception as e:
            logger.error(f"Failed to connect to {broker_type} broker: {e}")
            raise
    
    def _on_connect(self, client, userdata, flags, rc, properties, broker_type: str):
        """Handle MQTT connection."""
        if rc == 0:
            logger.info(f"Successfully connected to {broker_type} broker")
            # Re-establish subscriptions after a (re)connect. paho's loop does not
            # restore them automatically, so on a reconnect they would otherwise be
            # silently lost (M11). No-op on the first connect (nothing tracked yet).
            self._resubscribe(client, broker_type)
        else:
            logger.error(f"Failed to connect to {broker_type} broker with code {rc}")

    def _resubscribe(self, client, broker_type: str) -> None:
        subs = self._subscriptions if broker_type == "local" else self._iot_core_subscriptions
        with self._lock:
            items = list(subs.items())
        if not items:
            return
        logger.info(f"Re-subscribing to {len(items)} {broker_type} topic(s) after connect")
        for topic, info in items:
            try:
                client.subscribe(topic, qos=info.get("qos", 0))
            except Exception as e:
                logger.error(f"Failed to re-subscribe to {broker_type} topic {topic}: {e}")
    
    def _on_disconnect(self, client, userdata, flags, rc, properties, broker_type: str):
        """Handle MQTT disconnection."""
        if rc == 0:
            logger.info(f"Clean disconnection from {broker_type} broker")
        else:
            logger.error(f"Unexpected disconnection from {broker_type} broker - code: {rc}")
            # Common disconnect codes:
            # 1: Unacceptable protocol version
            # 2: Identifier rejected
            # 3: Server unavailable
            # 4: Bad username or password
            # 5: Not authorized
        
        # Clear pending subscriptions on disconnect
        with self._lock:
            if broker_type == "local":
                for event in self._pending_local_subscriptions.values():
                    event.set()  # Unblock waiting subscriptions
                self._pending_local_subscriptions.clear()
                self._local_mid_to_topic.clear()
            else:
                for event in self._pending_iot_core_subscriptions.values():
                    event.set()  # Unblock waiting subscriptions
                self._pending_iot_core_subscriptions.clear()
                self._iot_core_mid_to_topic.clear()
    
    def _on_local_message(self, client, userdata, message: mqtt.MQTTMessage):
        """Handle message from local broker."""
        self._process_message(message, self._subscriptions, "local")
    
    def _on_local_subscribe(self, client, userdata, mid, granted_qos, properties):
        """Handle local broker subscription confirmation."""
        with self._lock:
            topic = self._local_mid_to_topic.pop(mid, None)
            if topic and topic in self._pending_local_subscriptions:
                event = self._pending_local_subscriptions.pop(topic)
                if 0x80 in granted_qos:  # Subscription failed
                    logger.error(f"Local broker subscription failed for topic: {topic}")
                else:
                    logger.debug(f"Local broker subscription confirmed for topic: {topic}")
                event.set()
    
    def _on_iot_core_subscribe(self, client, userdata, mid, granted_qos, properties):
        """Handle IoT Core broker subscription confirmation."""
        with self._lock:
            topic = self._iot_core_mid_to_topic.pop(mid, None)
            if topic and topic in self._pending_iot_core_subscriptions:
                event = self._pending_iot_core_subscriptions.pop(topic)
                if 0x80 in granted_qos:  # Subscription failed
                    logger.error(f"IoT Core broker subscription failed for topic: {topic}")
                else:
                    logger.debug(f"IoT Core broker subscription confirmed for topic: {topic}")
                event.set()
    
    def _on_iot_core_message(self, client, userdata, message: mqtt.MQTTMessage):
        """Handle message from IoT Core broker."""
        self._process_message(message, self._iot_core_subscriptions, "iotcore")
    
    def _process_message(self, message: mqtt.MQTTMessage, subscriptions: Dict, broker_type: str):
        """Process received MQTT message."""
        topic = message.topic
        payload_size = len(message.payload)
        logger.debug(f"Processing message from {broker_type} broker - topic: {topic}, size: {payload_size} bytes, QoS: {message.qos}")
        
        try:
            payload = message.payload.decode('utf-8')
            logger.debug(f"Decoded message payload from {broker_type} broker on topic {topic}")
            
            try:
                msg_json = json.loads(payload)
                msg = MessageBuilder.from_object(msg_json).build()
                logger.debug(f"Successfully parsed JSON message from {broker_type} broker")
            except json.JSONDecodeError:
                logger.debug(f"Message from {broker_type} broker is not JSON, treating as raw payload")
                msg = Message()
                msg.raw = payload
            
            # Check for response to pending request
            with self._lock:
                if topic in self._response_ious:
                    logger.debug(f"Message from {broker_type} broker matches pending request on topic {topic}")
                    iou = self._response_ious.pop(topic)
                    iou.set_result(msg)
                    logger.debug(f"Completed request-response for topic {topic} from {broker_type} broker")
                    return
            
            # Find matching subscription
            matched_subscription = False
            for topic_filter, sub_info in subscriptions.items():
                if self.topic_matches_sub(topic_filter, topic):
                    logger.debug(f"Message from {broker_type} broker matches subscription filter: {topic_filter}")
                    callback = sub_info['callback']
                    if callback:
                        logger.debug(f"Dispatching message to callback for {broker_type} topic: {topic} (filter: {topic_filter})")
                        self._executor.submit(callback, topic, msg)
                        matched_subscription = True
                    break
            
            if not matched_subscription:
                logger.debug(f"No subscription found for {broker_type} topic: {topic}")
                    
        except Exception as e:
            logger.error(f"Error processing message from {broker_type} broker on topic {topic}: {e}", exc_info=True)
            # Don't re-raise - this could cause disconnection
    
    def disconnect(self):
        """Disconnect from all brokers."""
        logger.info("Initiating STANDALONE mode broker disconnection")
        
        if self._local_client:
            logger.debug("Stopping local broker client loop and disconnecting")
            self._local_client.loop_stop()
            self._local_client.disconnect()
            self._local_client = None
            logger.info("Disconnected from local broker")
        
        if self._iot_core_client:
            logger.debug("Stopping IoT Core broker client loop and disconnecting")
            self._iot_core_client.loop_stop()
            self._iot_core_client.disconnect()
            self._iot_core_client = None
            logger.info("Disconnected from IoT Core broker")
        
        logger.debug("Shutting down message processing thread pool")
        self._executor.shutdown(wait=True)
        logger.info("STANDALONE mode disconnection completed - all brokers disconnected and resources cleaned up")
    
    def publish(self, topic: str, msg: Message):
        """Publish message to local broker."""
        if not self._local_client:
            logger.error(f"Cannot publish to local broker - client not initialized")
            raise RuntimeError("Local broker client not available")
        
        try:
            payload = msg.dumps()
            logger.debug(f"Publishing message to local broker - topic: {topic}, size: {len(payload)} bytes")
            result = self._local_client.publish(topic, payload)
            
            if result.rc == mqtt.MQTT_ERR_SUCCESS:
                logger.debug(f"Successfully published message to local broker topic: {topic}")
            else:
                logger.error(f"Failed to publish to local broker topic {topic} - error code: {result.rc}")
                
        except Exception as e:
            logger.error(f"Error publishing message to local broker topic {topic}: {e}")
            raise
    
    def publish_to_iot_core(self, topic: str, msg: Message, qos: QOS):
        """Publish message to IoT Core broker."""
        if not self._iot_core_client:
            logger.error(f"Cannot publish to IoT Core broker - client not initialized")
            raise RuntimeError("IoT Core broker client not available")
        
        try:
            payload = msg.dumps()
            mqtt_qos = 0 if qos == QOS.AT_MOST_ONCE else 1
            logger.debug(f"Publishing message to IoT Core broker - topic: {topic}, size: {len(payload)} bytes, QoS: {mqtt_qos}")
            
            result = self._iot_core_client.publish(topic, payload, qos=mqtt_qos)
            
            if result.rc == mqtt.MQTT_ERR_SUCCESS:
                logger.debug(f"Successfully published message to IoT Core broker topic: {topic} (QoS: {mqtt_qos})")
            else:
                logger.error(f"Failed to publish to IoT Core broker topic {topic} - error code: {result.rc}")
                
        except Exception as e:
            logger.error(f"Error publishing message to IoT Core broker topic {topic}: {e}")
            raise
    
    def subscribe(self, topic: str, callback: Callable[[str, Message], None], max_concurrency: int = None):
        """Subscribe to topic on local broker and wait for confirmation."""
        if not self._local_client:
            logger.error(f"Cannot subscribe to local broker - client not initialized")
            raise RuntimeError("Local broker client not available")
        
        logger.debug(f"Subscribing to local broker topic: {topic}")
        
        try:
            # Create event for this subscription
            subscription_event = threading.Event()
            with self._lock:
                self._pending_local_subscriptions[topic] = subscription_event
            
            # Send subscription request
            result = self._local_client.subscribe(topic)
            if result[0] != mqtt.MQTT_ERR_SUCCESS:
                # Clean up and raise error
                with self._lock:
                    self._pending_local_subscriptions.pop(topic, None)
                raise RuntimeError(f"Failed to send local subscription request: {result[0]}")
            
            # Store message ID to topic mapping
            with self._lock:
                self._local_mid_to_topic[result[1]] = topic
            
            # Block until SUBACK received or timeout
            if not subscription_event.wait(timeout=self._subscription_timeout):
                # Clean up and raise timeout error
                with self._lock:
                    self._pending_local_subscriptions.pop(topic, None)
                    self._local_mid_to_topic.pop(result[1], None)
                raise TimeoutError(f"Local subscription to {topic} timed out after {self._subscription_timeout} seconds")
            
            # Subscription confirmed, store it (qos retained for re-subscribe on reconnect)
            self._subscriptions[topic] = {'callback': callback, 'max_concurrency': max_concurrency, 'qos': 0}
            logger.debug(f"Successfully subscribed to local broker topic: {topic}")
            logger.debug(f"Local broker subscription count: {len(self._subscriptions)}")
                
        except Exception as e:
            logger.error(f"Error subscribing to local broker topic {topic}: {e}")
            raise
    
    def subscribe_to_iot_core(self, topic: str, callback: Callable[[str, Message], None], qos: QOS, max_concurrency: int = None):
        """Subscribe to topic on IoT Core broker and wait for confirmation."""
        if not self._iot_core_client:
            logger.error(f"Cannot subscribe to IoT Core broker - client not initialized")
            raise RuntimeError("IoT Core broker client not available")
        
        mqtt_qos = 0 if qos == QOS.AT_MOST_ONCE else 1
        logger.debug(f"Subscribing to IoT Core broker topic: {topic} (QoS: {mqtt_qos})")
        
        try:
            # Create event for this subscription
            subscription_event = threading.Event()
            with self._lock:
                self._pending_iot_core_subscriptions[topic] = subscription_event
            
            # Send subscription request
            result = self._iot_core_client.subscribe(topic, qos=mqtt_qos)
            if result[0] != mqtt.MQTT_ERR_SUCCESS:
                # Clean up and raise error
                with self._lock:
                    self._pending_iot_core_subscriptions.pop(topic, None)
                raise RuntimeError(f"Failed to send IoT Core subscription request: {result[0]}")
            
            # Store message ID to topic mapping
            with self._lock:
                self._iot_core_mid_to_topic[result[1]] = topic
            
            # Block until SUBACK received or timeout
            if not subscription_event.wait(timeout=self._subscription_timeout):
                # Clean up and raise timeout error
                with self._lock:
                    self._pending_iot_core_subscriptions.pop(topic, None)
                    self._iot_core_mid_to_topic.pop(result[1], None)
                raise TimeoutError(f"IoT Core subscription to {topic} timed out after {self._subscription_timeout} seconds")
            
            # Subscription confirmed, store it (qos retained for re-subscribe on reconnect)
            self._iot_core_subscriptions[topic] = {'callback': callback, 'max_concurrency': max_concurrency, 'qos': mqtt_qos}
            logger.debug(f"Successfully subscribed to IoT Core broker topic: {topic} (QoS: {mqtt_qos})")
            logger.debug(f"IoT Core broker subscription count: {len(self._iot_core_subscriptions)}")
                
        except Exception as e:
            logger.error(f"Error subscribing to IoT Core broker topic {topic}: {e}")
            raise
    
    def request(self, topic: str, msg: Message) -> Iou:
        """Send request to local broker and wait for response."""
        logger.debug(f"Sending request to local broker topic: {topic}")
        
        reply_topic = f"ggcommons/reply-{uuid.uuid4()}"
        iou = Iou()
        
        with self._lock:
            self._response_ious[reply_topic] = iou
        
        # Set reply-to header
        msg.get_header().reply_to = reply_topic
        
        # Subscribe to reply topic
        self.subscribe(reply_topic, None)
        
        # Publish request
        self.publish(topic, msg)
        
        logger.debug(f"Request sent to local broker, waiting for response on topic: {reply_topic}")
        return iou
    
    def request_from_iot_core(self, topic: str, msg: Message) -> Iou:
        """Send request to IoT Core broker and wait for response."""
        logger.debug(f"Sending request to IoT Core broker topic: {topic}")
        
        reply_topic = f"ggcommons/reply-{uuid.uuid4()}"
        iou = Iou()
        
        with self._lock:
            self._response_ious[reply_topic] = iou
        
        # Set reply-to header
        msg.get_header().reply_to = reply_topic
        
        # Subscribe to reply topic
        self.subscribe_to_iot_core(reply_topic, None, QOS.AT_MOST_ONCE)
        
        # Publish request
        self.publish_to_iot_core(topic, msg, QOS.AT_LEAST_ONCE)
        
        logger.debug(f"Request sent to IoT Core broker, waiting for response on topic: {reply_topic}")
        return iou
    
    def reply(self, request: Message, reply: Message):
        """Send reply to local broker."""
        reply_topic = request.get_header().reply_to
        if not reply_topic:
            logger.error("Cannot send reply - no reply-to topic in request")
            raise ValueError("Request message missing reply-to topic")
        
        # Correlate the reply with the request so the requester can match it.
        reply.set_correlation_id(request.get_correlation_id())
        logger.debug(f"Sending reply to local broker topic: {reply_topic}")
        self.publish(reply_topic, reply)

    def reply_to_iot_core(self, request: Message, reply: Message):
        """Send reply to IoT Core broker."""
        reply_topic = request.get_header().reply_to
        if not reply_topic:
            logger.error("Cannot send reply - no reply-to topic in request")
            raise ValueError("Request message missing reply-to topic")

        # Correlate the reply with the request so the requester can match it.
        reply.set_correlation_id(request.get_correlation_id())
        logger.debug(f"Sending reply to IoT Core broker topic: {reply_topic}")
        self.publish_to_iot_core(reply_topic, reply, QOS.AT_LEAST_ONCE)
    
    def get_native_client(self):
        """Get native MQTT clients."""
        return {'local': self._local_client, 'iot_core': self._iot_core_client}
    
    def publish_raw(self, topic: str, msg: dict):
        """Publish raw message to local broker."""
        if not self._local_client:
            raise RuntimeError("Local broker not configured")
        
        payload = json.dumps(msg)
        self._local_client.publish(topic, payload, qos=1)
    
    def publish_to_iot_core_raw(self, topic: str, msg: dict, qos: str):
        """Publish raw message to IoT Core broker."""
        if not self._iot_core_client:
            raise RuntimeError("IoT Core broker not configured")
        
        mqtt_qos = 0 if qos == QOS.AT_MOST_ONCE else 1
        payload = json.dumps(msg)
        self._iot_core_client.publish(topic, payload, qos=mqtt_qos)
    
    def unsubscribe(self, topic: str):
        """Unsubscribe from topic on local broker."""
        if self._local_client and topic in self._subscriptions:
            self._local_client.unsubscribe(topic)
            del self._subscriptions[topic]
    
    def unsubscribe_from_iot_core(self, topic: str):
        """Unsubscribe from topic on IoT Core broker."""
        if self._iot_core_client and topic in self._iot_core_subscriptions:
            self._iot_core_client.unsubscribe(topic)
            del self._iot_core_subscriptions[topic]
    
    def cancel_request(self, iou: Iou):
        """Cancel pending request to local broker."""
        topic = iou.get_user_data()
        with self._lock:
            self._response_ious.pop(topic, None)
        self.unsubscribe(topic)
    
    def cancel_request_from_iot_core(self, iou: Iou):
        """Cancel pending request to IoT Core broker."""
        topic = iou.get_user_data()
        with self._lock:
            self._response_ious.pop(topic, None)
        self.unsubscribe_from_iot_core(topic)
    

    
