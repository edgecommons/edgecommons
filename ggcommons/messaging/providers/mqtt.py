import concurrent.futures.thread
import json
import logging
import queue
from threading import Thread
from typing import Callable
from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.messaging_client import MessagingProvider
from ggcommons.messaging.message import Message, MessageBuilder
import paho.mqtt.client as mqtt
import re
import ssl

from ggcommons.utils.iou import Iou

logger = logging.getLogger("MqttProvider")


class SubscriptionInfo:
    def __init__(
        self,
        topic: str,
        msg_q: queue.Queue,
        callback: Callable[[str, Message], None],
        max_concurrency: int,
    ):
        self.topic_filter = topic
        self.msg_q = msg_q
        self.callback = callback
        self.max_concurrency = max_concurrency


class MqttProvider(MessagingProvider):
    """
    MqttProvider(host: str, port: int)

    Bases: MessagingProvider

    MQTT provider for publishing and subscribing to MQTT topics.

    Parameters:
        host: str
            The MQTT broker hostname
        port: int
            The MQTT broker port number

    Attributes:
        _subscription_info: dict
            Maps topic filters to SubscriptionInfo objects
        _response_ious: dict
            Maps request reply_to topics to Iou objects
        _responses: dict
            Maps request reply_to topics to received response messages
        _host: str
            The MQTT broker host
        _port: int
            The MQTT broker port
        _mqtt_client: mqtt.Client
            The Paho MQTT client instance

    Methods:
        _on_message(client, userdata, message)
            Callback for received MQTT messages
        _queue_processor(subscription_info)
            Thread target for processing subscription queues
        _on_connect(client, userdata, flags, rc)
            Callback for MQTT broker connection
        _on_disconnect(client, userdata, rc)
            Callback for MQTT broker disconnection
        _internal_publish(topic, msg, qos=QOS.AT_LEAST_ONCE)
            Publishes a message to an MQTT topic
        publish(topic, msg)
            Publishes a message to an MQTT topic
        publish_to_iot_core(topic, msg, qos)
            Publishes a message to an IoT Core MQTT topic
        _internal_subscribe(topic_filter, callback, max_concurrency=None)
            Subscribes to an MQTT topic filter
        subscribe(topic_filter, callback, max_concurrency=None)
            Subscribes to an MQTT topic filter
        subscribe_to_iot_core(...)
            Subscribes to an IoT Core MQTT topic filter
        unsubscribe(topic)
            Unsubscribes from an MQTT topic filter
        request(topic, msg) -> Iou
            Makes a request and returns an Iou future
        cancel_request(iou)
            Cancels a pending request
        reply(request, reply)
            Publishes a reply to a request
        and other IoT Core specific methods
    """

    def __init__(self, host: str, port: int, client_id: str, creds_dir: str = None):
        super().__init__()
        self._subscription_info = {}
        self._response_ious = {}
        self._response_locks = {}
        self._responses = {}
        self._host = host
        self._port = port
        self._client_id = client_id
        self._mqtt_client = mqtt.Client(
            mqtt.CallbackAPIVersion.VERSION2, client_id=self._client_id
        )
        if creds_dir is not None:
            self._tls_set_certs(creds_dir)
        self._mqtt_client.connect(host=self._host, port=self._port)
        self._mqtt_client.on_message = self._on_message
        self._mqtt_client.on_connect = self._on_connect
        self._mqtt_client.on_disconnect = self._on_disconnect
        self._mqtt_client.loop_start()

    def disconnect(self):
        subscriptions = list(self._subscription_info.values())
        for subscription in subscriptions:
            self.unsubscribe(subscription.topic_filter)
        self._subscription_info = None
        self._mqtt_client.loop_stop()
        self._mqtt_client.disconnect()
        self._mqtt_client = None
        self._response_ious = None
        self._responses = None
        self._response_locks = None

    def _tls_set_certs(self, creds_dir: str):
        key = f"{creds_dir}/{self._client_id}.private.key"
        cert = f"{creds_dir}/{self._client_id}.cert.pem"
        ca_cert = f"{creds_dir}/root-CA.crt"
        ssl_context = self._ssl_alpn(ca_cert, cert, key)
        self._mqtt_client.tls_set_context(ssl_context)

    def _ssl_alpn(self, ca_file, cert_file, key_file):
        try:
            logger.debug("open ssl version:{}".format(ssl.OPENSSL_VERSION))
            ssl_context = ssl.create_default_context()
            ssl_context.set_alpn_protocols(["x-amzn-mqtt-ca"])
            ssl_context.check_hostname = False
            ssl_context.verify_mode = ssl.CERT_NONE
            ssl_context.load_verify_locations(ca_file)
            ssl_context.load_cert_chain(cert_file, key_file)
            return ssl_context
        except Exception as e:
            print("exception ssl_alpn()")
            raise e

    def _on_message(self, client, userdata, message: mqtt.MQTTMessage):
        topic = message.topic
        logger.debug(f"Received message on topic: {topic}")
        msg_chars = message.payload.decode("utf-8")
        try:
            msg = MessageBuilder.build(json.loads(msg_chars), True)
        except json.decoder.JSONDecodeError:
            msg = MessageBuilder.build(msg_chars, False)
        for topic_filter in self._subscription_info:
            if MessagingProvider.topic_matches_sub(topic_filter, topic):
                topic_payload_tuple = (topic, msg)
                self._subscription_info[topic_filter].msg_q.put(topic_payload_tuple)
                break

    def _queue_processor(self, subscription_info: SubscriptionInfo):
        logger.debug(
            f"Starting queue monitoring for subscription on {subscription_info.topic_filter}"
        )
        with concurrent.futures.ThreadPoolExecutor(
            max_workers=subscription_info.max_concurrency
        ) as executor:
            while True:
                queue_obj = subscription_info.msg_q.get()
                if type(queue_obj) == int and queue_obj == -1:
                    break
                topic = re.sub("^iotcore/", "", queue_obj[0])
                received_payload = queue_obj[1]
                if topic in self._response_ious:
                    iou = self._response_ious[topic]
                    del self._response_ious[topic]
                    self.unsubscribe(topic)
                    iou.set_result(received_payload)
                else:
                    executor.submit(subscription_info.callback, topic, received_payload)

    def _on_connect(self, client, userdata, flags, reason_code, properties):
        logger.info(f"Connected to MQTT broker at {self._host}:{self._port} as {self._client_id}")

    def _on_disconnect(self, userdata, flags, reason, properties, other):
        logger.info(f"Disconnected from MQTT broker at {self._host}:{self._port}")

    def _internal_publish(self, topic: str, msg: Message, qos: str = QOS.AT_LEAST_ONCE):
        if qos == QOS.AT_MOST_ONCE:
            mqtt_qos = 0
        else:
            mqtt_qos = 1
        self._mqtt_client.publish(topic, json.dumps(msg.to_dict()), mqtt_qos)

    def _internal_publish_raw(self, topic: str, msg: dict, qos: str = QOS.AT_LEAST_ONCE):
        if qos == QOS.AT_MOST_ONCE:
            mqtt_qos = 0
        else:
            mqtt_qos = 1
        self._mqtt_client.publish(topic, json.dumps(msg), mqtt_qos)

    def publish(self, topic: str, msg: Message):
        self._internal_publish(topic, msg)

    def publish_to_iot_core(self, topic: str, msg: Message, qos: str):
        adjusted_topic = f"iotcore/{topic}"
        self._internal_publish(adjusted_topic, msg, qos)

    def publish_raw(self, topic: str, msg: dict):
        self._internal_publish_raw(topic, msg)

    def publish_to_iot_core_raw(self, topic: str, msg: dict, qos: str):
        adjusted_topic = f"iotcore/{topic}"
        self._internal_publish_raw(adjusted_topic, msg, qos)

    def _internal_subscribe(self, topic_filter: str, callback: Callable[[str, Message], None], max_concurrency: int = None):
        if topic_filter not in self._subscription_info:
            logger.debug(f"Subscribing to topic filter: {topic_filter}")
            sub_info = SubscriptionInfo(
                topic_filter, queue.Queue(), callback, max_concurrency
            )
            self._subscription_info[topic_filter] = sub_info
            self._mqtt_client.subscribe(topic_filter)
            Thread(target=self._queue_processor, args=(sub_info,)).start()

    def get_native_client(self):
        return self._mqtt_client

    def subscribe(
        self,
        topic_filter: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
    ):
        """
        subscribe(topic_filter: str, callback: Callable[[str, Message], None], max_concurrency: int = None)

        Subscribes to an MQTT topic filter.

        Parameters
        ----------
        topic_filter : str
            The topic filter to subscribe to
        callback : Callable[[str, Message], None]
            The callback function to invoke on messages
        max_concurrency : int, optional
            The maximum number of concurrent messages to allow, by default None

        Returns
        -------
        None

        Subscribes the client to a topic filter using the provided callback. Messages received on matching topics will be passed to the callback.
        """
        self._internal_subscribe(topic_filter, callback, max_concurrency)

    def subscribe_to_iot_core(
        self,
        topic_filter: str,
        callback: Callable[[str, Message], None],
        qos: str,
        max_concurrency: int = None,
    ):
        """
        subscribe_to_iot_core(topic_filter: str, callback: Callable[[str, Message], None], qos: str, max_concurrency: int = None)

        Subscribes to an IoT Core MQTT topic filter.

        Parameters
        ----------
        topic_filter : str
            The topic filter to subscribe to
        callback : Callable[[str, Message], None]
            The callback function to invoke on messages
        qos : str
            The quality of service level
        max_concurrency : int, optional
            The maximum number of concurrent messages to allow, by default None

        Returns
        -------
        None

        Subscribes the client to an IoT Core topic filter using the provided callback.
        Messages received on matching topics will be passed to the callback. The topic
        filter is prefixed with "iotcore/" before subscribing.
        """
        adjusted_topic = "iotcore/" + topic_filter
        self._internal_subscribe(adjusted_topic, callback, max_concurrency)

    def unsubscribe(self, topic: str):
        self._mqtt_client.unsubscribe(topic)
        self._subscription_info[topic].msg_q.put(-1)
        del self._subscription_info[topic]

    def unsubscribe_from_iot_core(self, topic: str):
        adjusted_topic = f"iotcore/{topic}"
        self._mqtt_client.unsubscribe(adjusted_topic)
        self._subscription_info[topic].msg_q.put(-1)
        del self._subscription_info[adjusted_topic]

    def request(self, topic: str, msg: Message) -> Iou:
        """
        request(self, topic: str, msg: Message) -> Iou

        This method makes a request to a specific topic and returns an Iou future object
        to allow asynchronous waiting for the response message.

        It takes in two required parameters - the topic string that the request
        message will be published to, and the Message object containing the request data.

        The method first uses the Message.make_request() method to generate a unique
        identifier string that will be used as the "reply-to" topic for the response.

        It then instantiates a new Iou object, passing in the reply-to topic, to
        represent the future response.

        The Iou object is stored in an internal dictionary mapped by reply-to topic,
        to allow later matching of responses.

        Next, the client is subscribed to the reply-to topic using the default None
        callback, to receive the response without processing.

        The request message is then published to the provided topic.

        Finally, the Iou future object is returned to the caller.

        Parameters
        ----------
        topic : str
            The topic to publish the request message to
        msg : Message
            The request data encapsulated in a Message object

        Returns
        -------
        Iou : Iou
            A future object representing the pending response

        """
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe(reply_to, None, 1)
        self.publish(topic, msg)
        return iou

    def cancel_request(self, iou: Iou):
        topic = iou.get_user_data()
        self.unsubscribe(topic)
        del self._response_ious[topic]

    def reply(self, request: Message, reply: Message):
        """
        reply(self, request: Message, reply: Message)

        Publishes a reply message to a request topic.

        Parameters
        ----------
        request : Message
            The original request message
        reply : Message
            The reply message

        Returns
        -------
        None

        Sets the correlation ID on the reply message to match
        the request, and publishes it to the request's reply-to topic.
        This allows the reply to be routed back to the requestor.
        """
        reply.set_correlation_id(request.get_correlation_id())
        self.publish(request.get_header().get_reply_to(), reply)

    def request_from_iot_core(self, topic: str, msg: Message) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self._internal_subscribe(reply_to, None, 1)
        self.publish_to_iot_core(topic, msg, QOS.AT_MOST_ONCE)
        return iou

    def cancel_request_from_iot_core(self, iou: Iou):
        topic = iou.get_user_data()
        self.unsubscribe_from_iot_core(topic)
        del self._response_ious[topic]

    def reply_to_iot_core(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish_to_iot_core(
            request.get_header().get_reply_to(), reply, QOS.AT_MOST_ONCE
        )
