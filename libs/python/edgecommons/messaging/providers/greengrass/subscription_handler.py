import abc
import concurrent.futures.thread
import logging
import queue
from threading import Lock, Thread, current_thread
from typing import Callable
from edgecommons.messaging.message import Message
from edgecommons.messaging.messaging_provider import DEFAULT_MAX_MESSAGES


logger = logging.getLogger("SubscriptionHandler")


class SubscriptionHandler(metaclass=abc.ABCMeta):
    _STOP = object()

    def __init__(
        self,
        topic_filter,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        self._topic_filter = topic_filter
        self._callback_func = callback
        # Bounded queue (drop on overflow) when max_messages > 0, else unbounded — parity with
        # the Rust (bounded mpsc) / TS (drop-on-overflow) providers.
        self._max_messages = max_messages if max_messages is not None else DEFAULT_MAX_MESSAGES
        self._queue = (
            queue.Queue(maxsize=self._max_messages)
            if self._max_messages and self._max_messages > 0
            else queue.Queue()
        )
        self._max_concurrency = max_concurrency
        self._closed = False
        self._close_lock = Lock()
        self._thread = Thread(target=self._process_queue, daemon=True)
        self._thread.start()

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
    def parse_raw_payload(self, event):
        pass

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(
            f"Stream error: {error} for topic filter {self._topic_filter}. Closing stream."
        )
        return True  # Return True to close stream, False to keep stream open.

    def on_stream_closed(self) -> None:
        self.close()
        logger.debug(f"Subscription stream on topic {self._topic_filter} closed")

    def close(self, timeout: float = 2.0) -> None:
        with self._close_lock:
            if self._closed:
                return
            self._closed = True
        try:
            self._queue.put_nowait(self._STOP)
        except queue.Full:
            try:
                self._queue.get_nowait()
            except queue.Empty:
                pass
            try:
                self._queue.put_nowait(self._STOP)
            except queue.Full:
                logger.warning(
                    f"Could not enqueue close marker for subscription worker on "
                    f"'{self._topic_filter}'"
                )
        if current_thread() is not self._thread:
            self._thread.join(timeout=timeout)

    def on_stream_event(self, event) -> None:
        if self._closed:
            return
        logger.debug(f"Received stream message on topic filter: {self._topic_filter}")
        try:
            parsed = self.parse_raw_payload(event)
            if parsed is None:
                return
            topic, received_payload = parsed
            topic_payload_tuple = (topic, received_payload)
            # Non-blocking enqueue: a full bounded queue drops the message with a warning rather
            # than blocking the IPC stream thread (parity with Rust/TS).
            try:
                self._queue.put_nowait(topic_payload_tuple)
            except queue.Full:
                logger.warning(
                    f"subscription queue full (max_messages={self._max_messages}) for filter "
                    f"'{self._topic_filter}'; dropping message on {topic}"
                )
            logger.debug(
                f"IPC: common: PubSubDataHandler: on_stream_event: subscribed message: {received_payload}"
            )
        except Exception as error:
            logger.error(
                f"Exception {error} decoding payload for topic filter {self._topic_filter}"
            )
            logger.error(
                "Probable cause: payload is not an EdgeCommons protobuf message"
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
                    if queue_obj is self._STOP:
                        break
                    topic = queue_obj[0]
                    msg = queue_obj[1]
                    executor.submit(self._invoke_callback, topic, msg)
                except Exception as e:
                    logger.warning(
                        f"Exception while processing message from subscription to '{self._topic_filter}': {e}"
                    )
        logger.info(
            f"Queue monitoring stopped for subscription on topic {self._topic_filter}"
        )
