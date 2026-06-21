import abc
import concurrent.futures.thread
import logging
import queue
from threading import Thread
from typing import Callable
from ggcommons.messaging.message import Message


logger = logging.getLogger("SubscriptionHandler")


class SubscriptionHandler(metaclass=abc.ABCMeta):
    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
    ):
        self._topic_filter = topic_filter
        self._callback_func = callback
        self._queue = queue.Queue()
        self._max_concurrency = max_concurrency
        Thread(target=self._process_queue).start()

    def _invoke_callback(self, topic: str, msg: Message) -> None:
        # Wrap the user/lib callback so an exception it throws can never escape the
        # worker thread. An uncaught callback error on this thread would otherwise
        # surface in the eventstream RPC layer and can wedge the nucleus's single IPC
        # event-loop thread under crash/restart churn. Log and suppress instead.
        try:
            self._callback_func(topic, msg)
        except Exception as error:  # noqa: BLE001 - never let a bad message kill the worker
            logger.error(
                f"Exception {error} in subscription callback for topic '{topic}'"
                f" (filter '{self._topic_filter}'); suppressing to keep the worker alive"
            )

    def get_topic_filter(self):
        return self._topic_filter

    def get_max_concurrency(self):
        return self._max_concurrency

    @abc.abstractmethod
    def parse_raw_payload(self, event) -> (str, dict):
        pass

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(
            f"Stream error: {error} for topic filter {self._topic_filter}. Keeping stream open."
        )
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
        with concurrent.futures.ThreadPoolExecutor(
            max_workers=self._max_concurrency
        ) as executor:
            while True:
                try:
                    queue_obj = self._queue.get()
                    if type(queue_obj) == int and queue_obj == -1:
                        break
                    topic = queue_obj[0]
                    msg = Message.from_object(queue_obj[1])
                    executor.submit(self._invoke_callback, topic, msg)
                except Exception as e:
                    logger.warning(
                        f"Exception while processing message from subscription to '{self._topic_filter}': {e}"
                    )
