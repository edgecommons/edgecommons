import json
import logging
from typing import Callable
from ggcommons.messaging.messaging_client import MessagingProvider
from ggcommons.messaging.message import Message, MessageBuilder
import paho.mqtt.client as mqtt

import uuid

from ggcommons.utils.iou import Iou

logger = logging.getLogger("MqttProvider")


class MqttProvider(MessagingProvider):

    def __init__(self, host: str, port: int):
        super().__init__()
        self._subscription_handlers = {}
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
        msg_chars = message.payload.decode("utf-8")
        try:
            msg = MessageBuilder.build(json.loads(msg_chars), True)
        except json.decoder.JSONDecodeError:
            msg = MessageBuilder.build(msg_chars, False)
        if topic in self._response_ious:
            logger.debug(f"Received reply message on topic: {topic}")
            iou = self._response_ious[topic]
            del self._response_ious[topic]
            self.unsubscribe(topic)
            iou.set_result(msg)
            # lock = self._response_locks[topic]
            # del self._response_locks[topic]
            # self.unsubscribe(topic)
            # self._responses[lock] = msg
            # lock.release()
        else:
            for handler_spec in self._subscription_handlers:
                if MessagingProvider.topic_matches_sub(handler_spec, topic):
                    self._subscription_handlers[handler_spec](topic, msg)
                    break

    def _on_connect(self, client, userdata, flags, rc):
        logger.info(f"Connected to MQTT broker at {self._host}:{self._port}")

    def _on_disconnect(self, client, userdata, rc):
        logger.error(f"Disconnected from MQTT broker at {self._host}:{self._port}")

    def publish(self, topic: str, msg: Message):
        self._mqtt_client.publish(topic, json.dumps(msg.to_dict()))

    def subscribe(self, topic: str, callback: Callable[[str, Message], None]):
        self._subscription_handlers[topic] = callback
        self._mqtt_client.subscribe(topic)

    def unsubscribe(self, topic: str):
        self._mqtt_client.unsubscribe(topic)
        del self._subscription_handlers[topic]

    # def request(self, topic: str, msg: Message) -> Lock:
    def request(self, topic: str, msg: Message) -> Iou:
        reply_to = msg.make_request()
        iou = Iou()
        self._response_ious[reply_to] = iou
        self.subscribe(reply_to, None)
        self.publish(topic, msg)
        return iou

    def reply(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish(request.get_header().get_reply_to(), reply)
