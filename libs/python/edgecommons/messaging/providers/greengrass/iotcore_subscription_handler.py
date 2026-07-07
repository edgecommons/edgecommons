import logging
from typing import Callable
from edgecommons.messaging.message import Message
from edgecommons.messaging.providers.greengrass.subscription_handler import (
    SubscriptionHandler,
)

logger = logging.getLogger("IoTCoreSubscriptionHandler")


class IoTCoreSubscriptionHandler(SubscriptionHandler):
    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        super().__init__(topic_filter, callback, max_concurrency, max_messages)

    def parse_raw_payload(self, event):
        try:
            message = Message.from_bytes(event.message.payload)
            logger.debug(
                "IoT Core: decoded EdgeCommons protobuf message on topic %s",
                event.message.topic_name,
            )
            return event.message.topic_name, message
        except ValueError as error:
            logger.warning(
                "Problem decoding IoT Core payload into EdgeCommons protobuf Message on topic %s: "
                "%s. Ignoring message.",
                event.message.topic_name,
                error,
            )
            return None
