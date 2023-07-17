import logging
from asyncio import Future
from typing import Callable
import json
from ggcommons.messaging.messaging_client import MessagingProvider
from ggcommons.messaging.message import Message
from ggcommons.messaging.message import MessageBuilder
from awsiot.greengrasscoreipc.clientv2 import GreengrassCoreIPCClientV2
from awsiot.greengrasscoreipc.model import (
    SubscriptionResponseMessage,
    PublishMessage,
    UnauthorizedError,
    BinaryMessage, IoTCoreMessage
)

logger = logging.getLogger("GreengrassIpcProvider")


class SubscriptionHandler:

    def __init__(self, topic_filter, callback: Callable[[str, Message], None]):
        self._topic_filter = topic_filter
        self._callback_func = callback
        self._ipc_stream = None

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(f"Ipc stream error: {error} for topic filter {self._topic_filter}")
        return True  # Return True to close stream, False to keep stream open.

    def on_stream_closed(self) -> None:
        pass

    def on_ipc_stream_event(self, event: SubscriptionResponseMessage) -> None:
        """
        Notice: Ignore error calling "received_payload: str = (event.binary_message.message).decode("utf-8")"
            - Reason: model.SubscriptionResponseMessage doesn't recognize type
              of event.binary_message.message properly when the message come from IoT Core using bridge.
            - Resolution: Ignore error as a workaround
        """
        logger.debug(f"Received message on topic filter: {self._topic_filter}")

        try:
            if event.binary_message is None:
                received_payload = event.json_message.message
                received_topic = event.json_message.context.topic
            else:
                received_payload = json.loads(event.binary_message.message)
                received_topic = event.binary_message.context.topic
            logger.debug(f"ipc: common: PubSubDataHandler: on_stream_event: subscribed message: {received_payload}")
        except Exception as error:
            logger.error(f"Exception {error} decoding payload: {event.binary_message.message}")
            logger.error(f"Probable cause: common messaging library supports only json data")
            return

        self._callback_func(received_topic, MessageBuilder.build(received_payload))

    def on_iot_core_stream_event(self, event: IoTCoreMessage) -> None:
        pass


class GreengrassIpcProvider(MessagingProvider):

    def __init__(self, receive_own_messages: bool):
        super().__init__()
        self._subscription_handlers = {}
        self._subscription_operations = {}
        self._response_futures = {}
        self._receive_mode = 'RECEIVE_MESSAGES_FROM_OTHERS'
        if receive_own_messages:
            self._receive_mode = 'RECEIVE_ALL_MESSAGES'
        self._ipc_client = GreengrassCoreIPCClientV2()

    def publish(self, topic: str, msg: Message):
        msg_str = msg.dumps()
        self._ipc_client.publish_to_topic(topic=topic,
                                          publish_message=PublishMessage(binary_message=BinaryMessage(message=msg_str)))

    def subscribe(self, topic_filter: str, callback: Callable[[str, Message], None]):
        logger.info(f"Subscribing to ipc messages on topic {topic_filter}")
        handler = SubscriptionHandler(topic_filter, callback)
        try:
            _, operation = self._ipc_client.subscribe_to_topic(topic=topic_filter,
                                                               receive_mode=self._receive_mode,
                                                               on_stream_event=handler.on_ipc_stream_event,
                                                               on_stream_error=handler.on_stream_error,
                                                               on_stream_closed=handler.on_stream_closed)
            self._subscription_operations[topic_filter] = operation
            self._subscription_handlers[topic_filter] = handler
            logger.debug(f"Successfully subscribed to the topic filter: {topic_filter} on IPC channel")
        except UnauthorizedError:
            logger.error(f"Unauthorized error while subscribing to topic fitler {topic_filter}. "
                         f"Ensure access control policy is "
                         f"defined in the component configuration")
        except (ValueError, Exception) as error:
            logger.error(f"Unable to subscribe to topic filter ({topic_filter}): {error}")

    def unsubscribe(self, topic_filter: str):
        if topic_filter in self._subscription_operations:
            self._subscription_operations[topic_filter].close()
            del self._subscription_operations[topic_filter]
            del self._subscription_handlers[topic_filter]
        else:
            logger.warning(f"Attempt to unsubscribe from unknown topic {topic_filter}")

    def request(self, topic: str, msg: Message) -> Future:
        reply_to = msg.make_request()
        future = Future()
        self._response_futures[reply_to] = future
        self.subscribe(reply_to, self._on_reply_received)
        self.publish(topic, msg)
        return future

    def reply(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish(request.get_header().get_reply_to(), reply)

    def _on_reply_received(self, topic: str, reply: Message) -> None:
        if topic in self._response_futures:
            logger.info(f"Received reply message on topic: {topic}")
            future = self._response_futures[topic]
            del self._response_futures[topic]
            self.unsubscribe(topic)
            future.set_result(reply)

