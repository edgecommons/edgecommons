import threading
import time
from asyncio import Future
from typing import Callable, List
import logging
from ggcommons.messaging.message import Message
from ggcommons.messaging.messaging_provider import MessagingProvider
from ggcommons.messaging.providers.greengrass_ipc import GreengrassIpcProvider
from ggcommons.messaging.providers.greengrass_ipc_threaded import GreengrassIpcThreadedProvider
from ggcommons.messaging.providers.mqtt import MqttProvider
from ggcommons.utils.iou import Iou

logger = logging.getLogger("MessagingClient")


class MessagingClient:

    _messaging_provider = None

    @staticmethod
    def init(messaging_args: List[str], use_threaded_ipc=False, receive_own_messages=False) -> MessagingProvider:
        if messaging_args[0].upper() == "MQTT":
            logger.info("Using MqttClient")
            host = messaging_args[1] if len(messaging_args) > 1 else "localhost"
            port = messaging_args[2] if len(messaging_args) > 2 else 1883
            MessagingClient._messaging_provider = MqttProvider(host, port)
        else:
            if not use_threaded_ipc:
                logger.info("Using Greengrass IPC.")
                MessagingClient._messaging_provider = GreengrassIpcProvider(receive_own_messages)
            else:
                logger.info("Using Threaded Greengrass IPC.")
                threading.Thread(target=MessagingClient.create_ipc_threaded_provider,
                                 args=(receive_own_messages,),
                                 daemon=True,
                                 name="GGThreadedIpcProvider").start()
        if MessagingClient._messaging_provider is None:
            logger.fatal("Unable to create messaging provider.  Terminating.")
        return MessagingClient._messaging_provider

    @staticmethod
    def create_ipc_threaded_provider(receive_own_messages: bool):
        MessagingClient._messaging_provider = GreengrassIpcThreadedProvider(receive_own_messages)

    @staticmethod
    def publish(topic: str, msg: Message):
        MessagingClient._messaging_provider.publish(topic, msg)

    @staticmethod
    def subscribe(topic: str, callback: Callable[[str, Message], None]):
        MessagingClient._messaging_provider.subscribe(topic, callback)

    @staticmethod
    def unsubscribe(topic: str):
        MessagingClient._messaging_provider.unsubscribe(topic)

    @staticmethod
    def request(topic: str, msg: Message) -> Iou:
        return MessagingClient._messaging_provider.request(topic, msg)

    @staticmethod
    def get_reply(lock: threading.Lock) -> Message:
        return MessagingClient._messaging_provider.get_response(lock)

    @staticmethod
    def reply(request: Message, reply: Message):
        MessagingClient._messaging_provider.reply(request, reply)

    @staticmethod
    def topic_matches_sub(sub: str, topic: str) -> bool:
        return MessagingProvider.topic_matches_sub(sub, topic)
