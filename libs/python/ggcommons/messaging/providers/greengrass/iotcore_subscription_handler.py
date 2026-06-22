import json
import logging
from typing import Callable
from ggcommons.messaging.message import Message
from ggcommons.messaging.providers.greengrass.subscription_handler import (
    SubscriptionHandler,
)

logger = logging.getLogger("IotCoreSubscriptionHandler")


class IotCoreSubscriptionHandler(SubscriptionHandler):
    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        super().__init__(topic_filter, callback, max_concurrency, max_messages)

    def parse_raw_payload(self, event) -> (str, dict):
        received_payload = json.loads(str(event.message.payload, "utf-8"))
        logger.debug(
            f"IoT Core: common: PubSubDataHandler: on_stream_event: subscribed message: {received_payload}"
        )
        return event.message.topic_name, received_payload
