import threading
import logging
from typing import Callable, List

from ggcommons.messaging.message import Message
from ggcommons.messaging.messaging_provider import MessagingProvider
from ggcommons.messaging.providers.greengrass.greengrass_ipc import (
    GreengrassIpcProvider,
)
from ggcommons.messaging.providers.mqtt import MqttProvider
from ggcommons.utils.iou import Iou
from awsiot.greengrasscoreipc.model import QOS

logger = logging.getLogger("MessagingClient")


class MessagingClient:
    _messaging_provider: MessagingProvider = None

    @staticmethod
    def init(
        messaging_args: List[str], receive_own_messages=False
    ) -> MessagingProvider:
        if messaging_args[0].upper() == "MQTT":
            logger.info("Using MqttClient")
            host = messaging_args[1] if len(messaging_args) > 1 else "localhost"
            port = messaging_args[2] if len(messaging_args) > 2 else 1883
            MessagingClient._messaging_provider = MqttProvider(host, port)
        else:
            logger.info("Using Greengrass IPC.")
            MessagingClient._messaging_provider = GreengrassIpcProvider(
                receive_own_messages
            )
        if MessagingClient._messaging_provider is None:
            logger.fatal("Unable to create messaging provider.  Terminating.")
        return MessagingClient._messaging_provider

    @staticmethod
    def publish(topic: str, msg: Message):
        MessagingClient._messaging_provider.publish(topic, msg)

    @staticmethod
    def publish_raw(topic: str, msg: dict):
        MessagingClient._messaging_provider.publish_raw(topic, msg)

    @staticmethod
    def publish_to_iot_core(topic: str, msg: Message, qos: str):
        MessagingClient._messaging_provider.publish_to_iot_core(topic, msg, qos)

    @staticmethod
    def publish_to_iot_core_raw(topic: str, msg: dict, qos: str):
        MessagingClient._messaging_provider.publish_to_iot_core_raw(topic, msg, qos)

    @staticmethod
    def subscribe(
        topic: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
    ):
        MessagingClient._messaging_provider.subscribe(
            topic, callback, max_concurrency
        )

    @staticmethod
    def subscribe_to_iot_core(
        topic: str,
        callback: Callable[[str, Message], None],
        qos: QOS,
        max_concurrency: int = None,
    ):
        MessagingClient._messaging_provider.subscribe_to_iot_core(
            topic, callback, qos, max_concurrency
        )

    @staticmethod
    def unsubscribe(topic: str):
        MessagingClient._messaging_provider.unsubscribe(topic)

    @staticmethod
    def unsubscribe_from_iot_core(topic: str):
        MessagingClient._messaging_provider.unsubscribe_from_iot_core(topic)

    @staticmethod
    def request(topic: str, msg: Message) -> Iou:
        return MessagingClient._messaging_provider.request(topic, msg)

    @staticmethod
    def request_from_iot_core(topic: str, msg: Message) -> Iou:
        return MessagingClient._messaging_provider.request_from_iot_core(topic, msg)

    @staticmethod
    def cancel_request(iou: Iou) -> Iou:
        return MessagingClient._messaging_provider.cancel_request(iou)

    @staticmethod
    def cancel_request_from_iot_core(iou: Iou) -> Iou:
        return MessagingClient._messaging_provider.cancel_request(iou)

    @staticmethod
    def reply(request: Message, reply: Message):
        MessagingClient._messaging_provider.reply(request, reply)

    def reply_to_iot_core(request: Message, reply: Message):
        MessagingClient._messaging_provider.reply(request, reply)

    @staticmethod
    def topic_matches_sub(sub: str, topic: str) -> bool:
        return MessagingProvider.topic_matches_sub(sub, topic)

    @staticmethod
    def get_native_client():
        return MessagingProvider.get_native_client()
