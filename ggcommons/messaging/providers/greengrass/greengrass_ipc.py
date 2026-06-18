import logging
import json
from typing import Callable
from ggcommons.messaging.messaging_client import MessagingProvider
from ggcommons.messaging.message import Message
from awsiot.greengrasscoreipc.clientv2 import GreengrassCoreIPCClientV2
from awsiot.greengrasscoreipc.model import (
    PublishMessage,
    UnauthorizedError,
    BinaryMessage,
    QOS,
    JsonMessage,
)
from ggcommons.messaging.providers.greengrass.iotcore_subscription_handler import (
    IotCoreSubscriptionHandler,
)
from ggcommons.messaging.providers.greengrass.ipc_subscription_handler import (
    IpcSubscriptionHandler,
)
from ggcommons.utils.iou import Iou

logger = logging.getLogger("GreengrassIpcProvider")


class GreengrassIpcProvider(MessagingProvider):
    def __init__(self, receive_own_messages: bool):
        super().__init__()
        self._ipc_subscription_handlers = {}
        self._ipc_subscription_operations = {}
        self._iot_core_subscription_handlers = {}
        self._iot_core_subscription_operations = {}
        self._response_ious = {}
        self._receive_mode = "RECEIVE_MESSAGES_FROM_OTHERS"
        if receive_own_messages:
            self._receive_mode = "RECEIVE_ALL_MESSAGES"
        self._ipc_client = GreengrassCoreIPCClientV2()

    def disconnect(self):
        # The handler maps are keyed by topic filter, so iterate the keys directly
        # and unsubscribe on the matching transport.
        for topic_filter in list(self._ipc_subscription_handlers):
            self.unsubscribe(topic_filter)
        for topic_filter in list(self._iot_core_subscription_handlers):
            self.unsubscribe_from_iot_core(topic_filter)
        self._ipc_client.client.close()
        self._ipc_client = None

    def publish(self, topic: str, msg: Message):
        msg_str = msg.dumps()
        self._ipc_client.publish_to_topic(
            topic=topic,
            publish_message=PublishMessage(
                binary_message=BinaryMessage(message=msg_str)
            ),
        )

    def publish_raw(self, topic: str, msg: dict):
        self._ipc_client.publish_to_topic(
            topic=topic,
            publish_message=PublishMessage(json_message=JsonMessage(message=msg)),
        )

    def publish_to_iot_core(self, topic: str, msg: Message, qos: str):
        payload = msg.dumps()
        self._ipc_client.publish_to_iot_core(topic_name=topic, payload=payload, qos=qos)

    def publish_to_iot_core_raw(self, topic: str, msg: dict, qos: str):
        payload = json.dumps(msg)
        self._ipc_client.publish_to_iot_core(topic_name=topic, payload=payload, qos=qos)

    def subscribe(
        self,
        topic_filter: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
    ):
        logger.info(f"Subscribing to IPC messages on topic {topic_filter}")
        handler = IpcSubscriptionHandler(topic_filter, callback, max_concurrency)
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

    def subscribe_to_iot_core(
        self,
        topic_filter: str,
        callback: Callable[[str, Message], None],
        qos: str,
        max_concurrency: int = None,
    ):
        logger.info(f"Subscribing to iot core messages on topic {topic_filter}")
        handler = IotCoreSubscriptionHandler(topic_filter, callback, max_concurrency)
        try:
            _, operation = self._ipc_client.subscribe_to_iot_core(
                topic_name=topic_filter,
                qos=qos,
                on_stream_event=handler.on_stream_event,
                on_stream_error=handler.on_stream_error,
                on_stream_closed=handler.on_stream_closed,
            )
            self._iot_core_subscription_operations[topic_filter] = operation
            self._iot_core_subscription_handlers[topic_filter] = handler
            logger.debug(
                f"Successfully subscribed to the topic filter: {topic_filter} on IoT Core"
            )
        except UnauthorizedError:
            logger.error(
                f"Unauthorized error while subscribing to topic filter {topic_filter} on IoT Core. "
                f"Ensure access control policy is "
                f"defined in the component configuration"
            )
        except (ValueError, Exception) as error:
            logger.error(
                f"Unable to subscribe to IoT Core topic filter ({topic_filter}): {error}"
            )

    def unsubscribe(self, topic_filter: str):
        if topic_filter in self._ipc_subscription_operations:
            self._ipc_subscription_operations[topic_filter].close()
            del self._ipc_subscription_operations[topic_filter]
            del self._ipc_subscription_handlers[topic_filter]
        else:
            logger.warning(
                f"Attempt to unsubscribe from unknown IPC topic {topic_filter}"
            )

    def unsubscribe_from_iot_core(self, topic_filter: str):
        if topic_filter in self._iot_core_subscription_operations:
            self._iot_core_subscription_operations[topic_filter].close()
            del self._iot_core_subscription_operations[topic_filter]
            del self._iot_core_subscription_handlers[topic_filter]
        else:
            logger.warning(
                f"Attempt to unsubscribe from unknown IoT Core topic {topic_filter}"
            )

    def request(self, topic: str, msg: Message) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe(reply_to, self._on_reply_received, 1)
        self.publish(topic, msg)
        return iou

    def cancel_request(self, iou: Iou):
        reply_to = iou.get_user_data()
        self.unsubscribe(reply_to)
        del self._response_ious[reply_to]

    def reply(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish(request.get_header().get_reply_to(), reply)

    def _on_reply_received(self, topic: str, reply: Message) -> None:
        if topic in self._response_ious:
            logger.debug(f"Received reply message on topic: {topic}")
            iou = self._response_ious[topic]
            del self._response_ious[topic]
            self.unsubscribe(topic)
            iou.set_result(reply)

    def _on_iot_core_reply_received(self, topic: str, reply: Message) -> None:
        if topic in self._response_ious:
            logger.debug(f"Received IoT Core reply message on topic: {topic}")
            iou = self._response_ious[topic]
            del self._response_ious[topic]
            self.unsubscribe_from_iot_core(topic)
            iou.set_result(reply)

    def request_from_iot_core(self, topic: str, msg: Message) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe_to_iot_core(
            reply_to, self._on_iot_core_reply_received, QOS.AT_MOST_ONCE, 1
        )
        self.publish_to_iot_core(topic, msg, QOS.AT_MOST_ONCE)
        return iou

    def reply_to_iot_core(self, request: Message, reply: Message):
        reply.set_correlation_id(request.get_correlation_id())
        self.publish_to_iot_core(
            request.get_header().get_reply_to(), reply, QOS.AT_MOST_ONCE
        )

    def cancel_request_from_iot_core(self, iou: Iou):
        reply_to = iou.get_user_data()
        self.unsubscribe_from_iot_core(reply_to)
        del self._response_ious[reply_to]

    def get_native_client(self):
        return self._ipc_client
