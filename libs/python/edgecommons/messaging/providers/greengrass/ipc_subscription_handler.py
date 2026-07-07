import logging
from typing import Callable
from awsiot.greengrasscoreipc.model import SubscriptionResponseMessage
from edgecommons.messaging.message import Message
from edgecommons.messaging.providers.greengrass.subscription_handler import (
    SubscriptionHandler,
)

logger = logging.getLogger("IpcSubscriptionHandler")


class IpcSubscriptionHandler(SubscriptionHandler):
    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        super().__init__(topic_filter, callback, max_concurrency, max_messages)

    def parse_raw_payload(self, event: SubscriptionResponseMessage):
        if event.binary_message is None:
            topic = event.json_message.context.topic
            logger.warning(
                "Received Greengrass JsonMessage on EdgeCommons subscription topic %s; "
                "ignoring non-protobuf payload",
                topic,
            )
            return None
        else:
            topic = event.binary_message.context.topic
            try:
                return topic, Message.from_bytes(event.binary_message.message)
            except ValueError as error:
                logger.warning(
                    "Problem decoding IPC payload into EdgeCommons protobuf Message on topic %s: "
                    "%s. Ignoring message",
                    topic,
                    error,
                )
                return None
