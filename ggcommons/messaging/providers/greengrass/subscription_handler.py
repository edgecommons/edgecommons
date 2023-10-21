import abc
import logging
import queue
from threading import Thread
from typing import Callable
from ggcommons.messaging.message import Message
from ggcommons.messaging.message import MessageBuilder


logger = logging.getLogger("ConfigManager")


class SubscriptionHandler(metaclass=abc.ABCMeta):
    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        serialize_processing: bool = False,
    ):
        self._topic_filter = topic_filter
        self._callback_func = callback
        self._queue = queue.Queue()
        self._serialize_processing = serialize_processing
        Thread(target=self._process_queue).start()

    @abc.abstractmethod
    def parse_raw_payload(self, event) -> (str, dict):
        pass

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(f"Stream error: {error} for topic filter {self._topic_filter}")
        return True  # Return True to close stream, False to keep stream open.

    def on_stream_closed(self) -> None:
        # Puts a marker at the end of the queue processing thread to stop and join.
        # Note that existing messages in the queue will still process first.
        self._queue.put(-1)
        logger.debug(f"Subscription stream on topic {self._topic_filter} closed")

    def on_stream_event(self, event) -> None:
        logger.debug(f"Received stream message on topic filter: {self._topic_filter}")
        try:
            topic, received_payload = self.parse_raw_payload(event)
            topic_payload_tuple = (topic, received_payload)
            self._queue.put(topic_payload_tuple)
            logger.debug(
                f"IPC: common: PubSubDataHandler: on_stream_event: subscribed message: {received_payload}"
            )
        except Exception as error:
            logger.error(
                f"Exception {error} decoding payload: {event.binary_message.message}"
            )
            logger.error(
                f"Probable cause: common messaging library supports only json data"
            )

    def _process_queue(self):
        logger.info(
            f"Starting queue monitoring for subscription on topic {self._topic_filter}"
        )
        while True:
            try:
                queue_obj = self._queue.get()
                if type(queue_obj) == int and queue_obj == -1:
                    break
                topic = queue_obj[0]
                msg = MessageBuilder.build(queue_obj[1])
                if self._serialize_processing:
                    self._callback_func(topic, msg)
                else:
                    Thread(target=self._callback_func, args=(topic, msg)).start()
            except Exception as e:
                logger.warning(
                    f"Exception while processing message from subscription to '{self._topic_filter}': {e}"
                )

