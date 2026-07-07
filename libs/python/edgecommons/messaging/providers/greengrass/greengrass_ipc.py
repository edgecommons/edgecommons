import logging
import json
import time
from typing import Callable, Optional
from edgecommons.messaging.messaging_provider import MessagingProvider
from edgecommons.messaging.message import Message
from edgecommons.messaging.qos import Qos
import awsiot.greengrasscoreipc
from awsiot.greengrasscoreipc.clientv2 import GreengrassCoreIPCClientV2
from awsiot.greengrasscoreipc.model import (
    PublishMessage,
    UnauthorizedError,
    BinaryMessage,
    QOS as GreengrassQOS,
    JsonMessage,
)
from edgecommons.messaging.providers.greengrass.iotcore_subscription_handler import (
    IoTCoreSubscriptionHandler,
)
from edgecommons.messaging.providers.greengrass.ipc_subscription_handler import (
    IpcSubscriptionHandler,
)
from edgecommons.utils.iou import Iou

logger = logging.getLogger("GreengrassIpcProvider")


class GreengrassIpcProvider(MessagingProvider):
    def __init__(self, receive_own_messages: bool):
        super().__init__()
        self._ipc_subscription_handlers = {}
        self._ipc_subscription_operations = {}
        self._northbound_subscription_handlers = {}
        self._northbound_subscription_operations = {}
        self._response_ious = {}
        self._receive_mode = "RECEIVE_MESSAGES_FROM_OTHERS"
        if receive_own_messages:
            self._receive_mode = "RECEIVE_ALL_MESSAGES"
        self._ipc_client = self._connect_with_retry()

    @staticmethod
    def _greengrass_qos(qos: Qos):
        if qos == Qos.AT_MOST_ONCE:
            return GreengrassQOS.AT_MOST_ONCE
        if qos == Qos.AT_LEAST_ONCE:
            return GreengrassQOS.AT_LEAST_ONCE
        raise ValueError("Greengrass IoT Core IPC supports only MQTT QoS 0 and 1; got EXACTLY_ONCE")

    @staticmethod
    def _connect_with_retry(attempts: int = 5, connect_timeout: float = 30.0):
        """Open the Greengrass IPC client, retrying on transient connect failures.

        A bare ``GreengrassCoreIPCClientV2()`` makes a single connect with a short (~10s) timeout
        and no retry, so a slow/cold SDK init or a momentarily busy nucleus aborts component
        startup. Build the underlying connection with a generous timeout and retry with backoff so
        the component comes up reliably.
        """
        last_err = None
        for attempt in range(1, attempts + 1):
            try:
                connection = awsiot.greengrasscoreipc.connect(timeout=connect_timeout)
                return GreengrassCoreIPCClientV2(connection)
            except Exception as e:  # noqa: BLE001 - surface the last error after exhausting retries
                last_err = e
                logger.warning(
                    f"Greengrass IPC connect attempt {attempt}/{attempts} failed: {e}"
                    + (f"; retrying in {attempt}s" if attempt < attempts else "")
                )
                if attempt < attempts:
                    time.sleep(attempt)
        raise RuntimeError(f"Greengrass IPC connect failed after {attempts} attempts: {last_err}")

    def connected(self) -> bool:
        """Report the IPC connection state for readiness (FR-HB-1).

        For GREENGRASS/IPC, ``connected()`` is ``True`` once the IPC client is built (the
        ``GreengrassCoreIPCClientV2`` connects with retry during construction); it becomes ``False``
        after :meth:`disconnect` nulls the client.
        """
        return self._ipc_client is not None

    def disconnect(self):
        # The handler maps are keyed by topic filter, so iterate the keys directly
        # and unsubscribe on the matching transport.
        for topic_filter in list(self._ipc_subscription_handlers):
            self.unsubscribe(topic_filter)
        for topic_filter in list(self._northbound_subscription_handlers):
            self.unsubscribe_northbound(topic_filter)
        if self._ipc_client is not None:
            self._ipc_client.client.close()
            self._ipc_client = None

    def publish(self, topic: str, msg: Message):
        payload = msg.to_bytes()
        self._ipc_client.publish_to_topic(
            topic=topic,
            publish_message=PublishMessage(
                binary_message=BinaryMessage(message=payload)
            ),
        )

    def publish_raw(self, topic: str, msg: dict):
        self._ipc_client.publish_to_topic(
            topic=topic,
            publish_message=PublishMessage(json_message=JsonMessage(message=msg)),
        )

    def publish_northbound(self, topic: str, msg: Message, qos: Qos):
        payload = msg.to_bytes()
        self._ipc_client.publish_to_iot_core(
            topic_name=topic, payload=payload, qos=self._greengrass_qos(qos)
        )

    def publish_northbound_raw(self, topic: str, msg: dict, qos: Qos):
        payload = json.dumps(msg)
        self._ipc_client.publish_to_iot_core(
            topic_name=topic, payload=payload, qos=self._greengrass_qos(qos)
        )

    def subscribe(
        self,
        topic_filter: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        logger.info(f"Subscribing to IPC messages on topic {topic_filter}")
        handler = IpcSubscriptionHandler(topic_filter, callback, max_concurrency, max_messages)
        try:
            _, operation = self._ipc_client.subscribe_to_topic(
                topic=topic_filter,
                receive_mode=self._receive_mode,
                on_stream_event=handler.on_stream_event,
                on_stream_error=handler.on_stream_error,
                on_stream_closed=handler.on_stream_closed,
            )
            self._ipc_subscription_operations[topic_filter] = operation
            self._ipc_subscription_handlers[topic_filter] = handler
            logger.debug(
                f"Successfully subscribed to the topic filter: {topic_filter} on IPC channel"
            )
        except UnauthorizedError:
            logger.error(
                f"Unauthorized error while subscribing to topic filter {topic_filter} on IPC. "
                f"Ensure access control policy is "
                f"defined in the component configuration"
            )
        except (ValueError, Exception) as error:
            logger.error(
                f"Unable to subscribe to IPC topic filter ({topic_filter}): {error}"
            )

    def subscribe_northbound(
        self,
        topic_filter: str,
        callback: Callable[[str, Message], None],
        qos: Qos,
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        logger.info(f"Subscribing to northbound messages on topic {topic_filter}")
        handler = IoTCoreSubscriptionHandler(topic_filter, callback, max_concurrency, max_messages)
        try:
            _, operation = self._ipc_client.subscribe_to_iot_core(
                topic_name=topic_filter,
                qos=self._greengrass_qos(qos),
                on_stream_event=handler.on_stream_event,
                on_stream_error=handler.on_stream_error,
                on_stream_closed=handler.on_stream_closed,
            )
            self._northbound_subscription_operations[topic_filter] = operation
            self._northbound_subscription_handlers[topic_filter] = handler
            logger.debug(
                f"Successfully subscribed to the topic filter: {topic_filter} on northbound"
            )
        except UnauthorizedError:
            logger.error(
                f"Unauthorized error while subscribing to topic filter {topic_filter} on northbound. "
                f"Ensure access control policy is "
                f"defined in the component configuration"
            )
        except (ValueError, Exception) as error:
            logger.error(
                f"Unable to subscribe to northbound topic filter ({topic_filter}): {error}"
            )

    def unsubscribe(self, topic_filter: str):
        if topic_filter in self._ipc_subscription_operations:
            operation = self._ipc_subscription_operations[topic_filter]
            handler = self._ipc_subscription_handlers[topic_filter]
            operation.close()
            handler.close()
            del self._ipc_subscription_operations[topic_filter]
            del self._ipc_subscription_handlers[topic_filter]
        else:
            logger.warning(
                f"Attempt to unsubscribe from unknown IPC topic {topic_filter}"
            )

    def unsubscribe_northbound(self, topic_filter: str):
        if topic_filter in self._northbound_subscription_operations:
            operation = self._northbound_subscription_operations[topic_filter]
            handler = self._northbound_subscription_handlers[topic_filter]
            operation.close()
            handler.close()
            del self._northbound_subscription_operations[topic_filter]
            del self._northbound_subscription_handlers[topic_filter]
        else:
            logger.warning(
                f"Attempt to unsubscribe from unknown northbound topic {topic_filter}"
            )

    def request(self, topic: str, msg: Message, timeout_secs: Optional[float] = None) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe(reply_to, self._on_reply_received, 1)

        # Arm the framework-owned deadline at send time (UNS-CANONICAL-DESIGN §5): on
        # expiry the timer unsubscribes the ephemeral reply topic, removes the pending
        # entry and completes the future exceptionally (RequestTimeoutError).
        def _deadline_cleanup():
            self._response_ious.pop(reply_to, None)
            self.unsubscribe(reply_to)

        self._arm_request_deadline(iou, self._effective_request_timeout(timeout_secs),
                                   _deadline_cleanup)
        self.publish(topic, msg)
        return iou

    def cancel_request(self, iou: Iou):
        if not iou.try_settle():
            return  # reply or deadline already settled + cleaned up this request
        reply_to = iou.get_user_data()
        self.unsubscribe(reply_to)
        self._response_ious.pop(reply_to, None)
        iou.set_result(None)

    def reply(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish(request.get_header().get_reply_to(), reply)

    def _on_reply_received(self, topic: str, reply: Message) -> None:
        # Reply arrival: race the single idempotent settle path (§5.1) against the
        # framework deadline and cancel_request. The winner owns the cleanup and the
        # completion; a loser (straggler/duplicate reply after settle) is dropped.
        iou = self._response_ious.get(topic)
        if iou is None or not iou.try_settle():
            logger.debug(f"Dropping straggler reply on '{topic}' (request already settled)")
            return
        logger.debug(f"Received reply message on topic: {topic}")
        self.unsubscribe(topic)
        self._response_ious.pop(topic, None)
        iou.set_result(reply)

    def _on_northbound_reply_received(self, topic: str, reply: Message) -> None:
        # Same single idempotent settle path as _on_reply_received (§5.1).
        iou = self._response_ious.get(topic)
        if iou is None or not iou.try_settle():
            logger.debug(f"Dropping straggler reply on '{topic}' (request already settled)")
            return
        logger.debug(f"Received northbound reply message on topic: {topic}")
        self.unsubscribe_northbound(topic)
        self._response_ious.pop(topic, None)
        iou.set_result(reply)

    def request_northbound(self, topic: str, msg: Message,
                           timeout_secs: Optional[float] = None) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe_northbound(
            reply_to, self._on_northbound_reply_received, Qos.AT_MOST_ONCE, 1
        )

        def _deadline_cleanup():
            self._response_ious.pop(reply_to, None)
            self.unsubscribe_northbound(reply_to)

        self._arm_request_deadline(iou, self._effective_request_timeout(timeout_secs),
                                   _deadline_cleanup)
        self.publish_northbound(topic, msg, Qos.AT_MOST_ONCE)
        return iou

    def reply_northbound(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish_northbound(
            request.get_header().get_reply_to(), reply, Qos.AT_MOST_ONCE
        )

    def cancel_request_northbound(self, iou: Iou):
        if not iou.try_settle():
            return  # reply or deadline already settled + cleaned up this request
        reply_to = iou.get_user_data()
        self.unsubscribe_northbound(reply_to)
        self._response_ious.pop(reply_to, None)
        iou.set_result(None)

    def get_native_client(self):
        return self._ipc_client
