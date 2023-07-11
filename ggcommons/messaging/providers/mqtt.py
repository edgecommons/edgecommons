import json
import logging
from typing import Callable
from ggcommons.messaging.messaging_client import MessagingProvider
from ggcommons.messaging.message import Message, MessageBuilder
from ggcommons.config.manager.config_manager import ConfigManager
import paho.mqtt.client as mqtt

import uuid

logger = logging.getLogger("MqttProvider")


class MqttProvider(MessagingProvider):

    def __init__(self, host: str, port: int):
        super().__init__()
        self._subscription_handlers = {}
        self._host = host
        self._port = port
        self._mqtt_client = mqtt.Client(client_id=f"{uuid.uuid4()}")
        self._mqtt_client.connect(host=self._host, port=self._port)
        self._mqtt_client.on_message = self.on_message
        self._mqtt_client.loop_start()

    def on_message(self, client, userdata, message: mqtt.MQTTMessage):
        topic = message.topic
        msg_chars = message.payload.decode("utf-8")
        try:
            msg = MessageBuilder.build(json.loads(msg_chars), True)
        except json.decoder.JSONDecodeError:
            msg = MessageBuilder.build(msg_chars, False)
        for handler_spec in self._subscription_handlers:
            if MessagingProvider.topic_matches_sub(handler_spec, topic):
                self._subscription_handlers[handler_spec](topic, msg)
                break

    def publish(self, topic: str, msg: Message):
        self._mqtt_client.publish(topic, json.dumps(msg.to_dict()))

    def subscribe(self, topic: str, callback: Callable[[str, Message], None]):
        self._subscription_handlers[topic] = callback
        self._mqtt_client.subscribe(topic)

    def unsubscribe(self, topic: str):
        self._mqtt_client.unsubscribe(topic)
        del self._subscription_handlers[topic]
