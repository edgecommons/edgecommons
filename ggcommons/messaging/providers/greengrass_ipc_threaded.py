import logging
import threading
from typing import Callable
import json
import queue
from ggcommons.messaging.messaging_client import MessagingProvider
from ggcommons.messaging.message import Message
from ggcommons.messaging.message import MessageBuilder
from ggcommons.config.manager.config_manager import ConfigManager
from awsiot.greengrasscoreipc.clientv2 import GreengrassCoreIPCClientV2
from awsiot.greengrasscoreipc.model import (
    SubscriptionResponseMessage,
    PublishMessage,
    UnauthorizedError, BinaryMessage
)

logger = logging.getLogger("GreengrassIpcThreadedProvider")


class QueueSubscriptionHandler:

    def __init__(self, topic_filter, incoming_queue):
        self._topic_filter = topic_filter
        self._incoming_queue = incoming_queue

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(f"Ipc stream error: {error} for topic {self._topic_filter}")
        return True  # Return True to close stream, False to keep stream open.

    def on_stream_closed(self) -> None:
        pass

    def on_stream_event(self, event: SubscriptionResponseMessage) -> None:
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
            logger.debug(f"Received message: {received_payload}")
        except Exception as error:
            logger.error(f"Exception {error} decoding payload: {event.binary_message.message}")
            logger.error(f"Probable cause: common messaging library supports only json data")
            return

        self._incoming_queue.put((self._topic_filter, received_topic, MessageBuilder.build(received_payload)))
        logger.debug(f"Received message on topic '{received_topic}' placed in incoming queue")


class GreengrassIpcThreadedProvider(MessagingProvider):

    def __init__(self, receive_own_messages: bool):
        super().__init__()
        self._subscription_handlers = {}
        self._subscription_operations = {}
        self._subscription_callbacks = {}
        self._receive_mode = 'RECEIVE_MESSAGES_FROM_OTHERS'
        if receive_own_messages:
            self._receive_mode = 'RECEIVE_ALL_MESSAGES'
        self._incoming_queue = queue.Queue()
        self._ipc_client = GreengrassCoreIPCClientV2()
        self._queue_processing_thread = threading.Thread(target=self.incoming_message_queue_processor,
                                                         daemon=True,
                                                         name="IpcQueueProcessingThread").start()

    def publish(self, topic: str, msg: Message):
        # json_message = JsonMessage(message=msg.to_dict())
        # self._ipc_client.publish_to_topic(topic=topic,
        #                                   publish_message=PublishMessage(json_message=json_message))
        binary_message = BinaryMessage(message=msg.dumps())
        self._ipc_client.publish_to_topic(topic=topic,
                                          publish_message=PublishMessage(binary_message=binary_message))

    def subscribe(self, topic_filter: str, callback: Callable[[str, Message], None]):
        logger.info(f"Subscribing to ipc messages on topic filter {topic_filter}")
        if topic_filter in self._subscription_handlers:
            logger.warning(f"Attempt to subscribe to {topic_filter} more than once. Ignoring.")
            return
        handler = QueueSubscriptionHandler(topic_filter, self._incoming_queue)
        try:
            _, operation = self._ipc_client.subscribe_to_topic(topic=topic_filter,
                                                               receive_mode=self._receive_mode,
                                                               on_stream_event=handler.on_stream_event,
                                                               on_stream_error=handler.on_stream_error,
                                                               on_stream_closed=handler.on_stream_closed)
            self._subscription_operations[topic_filter] = operation
            self._subscription_handlers[topic_filter] = handler
            self._subscription_callbacks[topic_filter] = callback
            logger.debug(f"Successfully subscribed to the topic : {topic_filter}")
        except UnauthorizedError:
            logger.error(f"Unauthorized error while subscribing to topic {topic_filter}. Ensure access control policy is "
                         f"defined in the component configuration")
        except (ValueError, Exception) as error:
            logger.error(f"Unable to subscribe to topic filter ({topic_filter}): {error}")

    def unsubscribe(self, topic_filter: str):
        if topic_filter in self._subscription_operations:
            self._subscription_operations[topic_filter].close()
            del self._subscription_operations[topic_filter]
            del self._subscription_handlers[topic_filter]
            del self._subscription_callbacks[topic_filter]
        else:
            logger.warning(f"Attempt to unsubscribe from unknown topic filter {topic_filter}")

    def incoming_message_queue_processor(self):
        while True:
            try:
                (topic_filter, topic, msg) = self._incoming_queue.get()
                logger.debug(f"Processing message on topic_filter {topic_filter} (topic: {topic}) from internal queue")
                self._subscription_callbacks[topic_filter](topic, msg)
            except Exception as ex:
                logging.exception(ex)
