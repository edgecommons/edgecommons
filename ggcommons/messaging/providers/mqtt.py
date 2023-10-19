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

import uuid

from ggcommons.utils.iou import Iou

logger = logging.getLogger("MqttProvider")


class SubscriptionInfo:
    def __init__(self, topic: str, msg_q: queue.Queue, callback: Callable[[str, Message], None], serialize=False):
        self.topic_filter = topic
        self.msg_q = msg_q
        self.callback = callback
        self.serialize = serialize


class MqttProvider(MessagingProvider):
    def __init__(self, host: str, port: int):
        super().__init__()
        self._ipc_subscription_info = {}
        self._iot_core_subscription_info = {}
        self._response_ious = {}
        self._response_locks = {}
        self._responses = {}
        self._host = host
        self._port = port
        self._mqtt_client = mqtt.Client(client_id=f"{uuid.uuid4()}")
        self._mqtt_client.connect(host=self._host, port=self._port)
        self._mqtt_client.on_message = self._on_message
        self._mqtt_client.on_connect = self._on_connect
        self._mqtt_client.on_disconnect = self._on_disconnect
        self._mqtt_client.loop_start()

    def _on_message(self, client, userdata, message: mqtt.MQTTMessage):
        topic = message.topic
        logger.debug(f"Received message on topic: {topic}")
        msg_chars = message.payload.decode("utf-8")
        try:
            msg = MessageBuilder.build(json.loads(msg_chars), True)
        except json.decoder.JSONDecodeError:
            msg = MessageBuilder.build(msg_chars, False)
        if topic.startswith("iotcore/"):
            subscriptions = self._iot_core_subscription_info
        else:
            subscriptions = self._ipc_subscription_info
        for topic_filter in subscriptions:
            if MessagingProvider.topic_matches_sub(topic_filter, topic):
                topic_payload_tuple = (topic, msg)
                subscriptions[topic_filter].msg_q.put(topic_payload_tuple)
                break

    def _queue_processor(self, subscription_info: SubscriptionInfo):
        logger.debug(f"Starting queue monitoring for subscription on {subscription_info.topic_filter}")
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
            if subscription_info.serialize:
                subscription_info.callback(topic, received_payload)
            else:
                tmp_thread = Thread(
                    target=subscription_info.callback,
                    args=(topic, received_payload),
                )
                tmp_thread.start()

    def _on_connect(self, client, userdata, flags, rc):
        logger.info(f"Connected to MQTT broker at {self._host}:{self._port}")

    def _on_disconnect(self, client, userdata, rc):
        logger.error(f"Disconnected from MQTT broker at {self._host}:{self._port}")

    def _internal_publish(self, topic: str, msg: Message, qos: str = QOS.AT_LEAST_ONCE):
        if qos == QOS.AT_MOST_ONCE:
            mqtt_qos = 0
        else:
            mqtt_qos = 1
        self._mqtt_client.publish(topic, json.dumps(msg.to_dict()), mqtt_qos)

    def publish(self, topic: str, msg: Message):
        self._internal_publish(topic, msg)

    def publish_to_iot_core(self, topic: str, msg: Message, qos: str):
        adjusted_topic = f"iotcore/{topic}"
        self._internal_publish(adjusted_topic, msg, qos)

    def subscribe(self, topic_filter: str, callback: Callable[[str, Message], None], serialize_processing=False):
        if topic_filter not in self._ipc_subscription_info:
            logger.debug(f"Subscribing to topic filter: {topic_filter}")
            sub_info = SubscriptionInfo(topic_filter, queue.Queue(), callback, serialize_processing)
            self._ipc_subscription_info[topic_filter] = sub_info
            self._mqtt_client.subscribe(topic_filter)
            Thread(target=self._queue_processor, args=(sub_info,)).start()

    def subscribe_to_iot_core(
        self, topic_filter: str, callback: Callable[[str, Message], None], qos: str, serialize_processing=False
    ):
        adjusted_topic = "iotcore/" + topic_filter
        if adjusted_topic not in self._iot_core_subscription_info:
            sub_info = SubscriptionInfo(adjusted_topic, queue.Queue(), callback, serialize_processing)
            self._iot_core_subscription_info[adjusted_topic] = sub_info
            self._mqtt_client.subscribe(adjusted_topic)
            Thread(target=self._queue_processor, args=(sub_info,)).start()

    def unsubscribe(self, topic: str):
        self._mqtt_client.unsubscribe(topic)
        self._ipc_subscription_info[topic].msg_q.put(-1)
        del self._ipc_subscription_info[topic]

    def unsubscribe_from_iot_core(self, topic: str):
        adjusted_topic = f"iotcore/{topic}"
        self._mqtt_client.unsubscribe(adjusted_topic)
        self._iot_core_subscription_info[topic].msg_q.put(-1)
        del self._iot_core_subscription_info[adjusted_topic]

    def request(self, topic: str, msg: Message) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe(reply_to, None)
        self.publish(topic, msg)
        return iou

    def cancel_request(self, iou: Iou):
        topic = iou.get_user_data()
        self.unsubscribe(topic)
        del self._response_ious[topic]

    def reply(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish(request.get_header().get_reply_to(), reply)
