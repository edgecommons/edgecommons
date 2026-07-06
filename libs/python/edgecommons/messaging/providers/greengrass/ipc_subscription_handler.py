import json
from typing import Callable
from awsiot.greengrasscoreipc.model import SubscriptionResponseMessage
from edgecommons.messaging.message import Message
from edgecommons.messaging.providers.greengrass.subscription_handler import (
    SubscriptionHandler,
)


class IpcSubscriptionHandler(SubscriptionHandler):
    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        super().__init__(topic_filter, callback, max_concurrency, max_messages)

    def parse_raw_payload(self, event: SubscriptionResponseMessage) -> (str, dict):
        if event.binary_message is None:
            received_payload = event.json_message.message
            topic = event.json_message.context.topic
        else:
            received_payload = json.loads(event.binary_message.message)
            topic = event.binary_message.context.topic
        return topic, received_payload
