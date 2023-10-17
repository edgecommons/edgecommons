import logging
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
    BinaryMessage, IoTCoreMessage, QOS
)

from ggcommons.utils.iou import Iou

logger = logging.getLogger("GreengrassIpcProvider")


class IpcSubscriptionHandler:

    def __init__(self, topic_filter, callback: Callable[[str, Message], None]):
        self._topic_filter = topic_filter
        self._callback_func = callback

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(f"IPC stream error: {error} for topic filter {self._topic_filter}")
        return True  # Return True to close stream, False to keep stream open.

    def on_stream_closed(self) -> None:
        pass

    def on_stream_event(self, event: SubscriptionResponseMessage) -> None:
        logger.debug(f"Received ipc message on topic filter: {self._topic_filter}")
        try:
            if event.binary_message is None:
                received_payload = event.json_message.message
                received_topic = event.json_message.context.topic
            else:
                received_payload = json.loads(event.binary_message.message)
                received_topic = event.binary_message.context.topic
            logger.debug(f"IPC: common: PubSubDataHandler: on_stream_event: subscribed message: {received_payload}")
        except Exception as error:
            logger.error(f"Exception {error} decoding payload: {event.binary_message.message}")
            logger.error(f"Probable cause: common messaging library supports only json data")
            return
        self._callback_func(received_topic, MessageBuilder.build(received_payload))


class IoTCoreSubscriptionHandler:

    def __init__(self, topic_filter, callback: Callable[[str, Message], None]):
        self._topic_filter = topic_filter
        self._callback_func = callback

    def on_stream_error(self, error: Exception) -> bool:
        logger.error(f"IOT Core Stream error: {error} for topic filter {self._topic_filter}")
        return True  # Return True to close stream, False to keep stream open.

    def on_stream_closed(self) -> None:
        pass

    def on_stream_event(self, event) -> None:
        logger.debug(f"Received message on IoT Core topic filter: {self._topic_filter}")
        try:
            received_topic = event.message.topic_name
            received_payload = json.loads(str(event.message.payload, 'utf-8'))
            logger.debug(f"IoT Core: common: PubSubDataHandler: on_stream_event: subscribed message: {received_payload}")
        except Exception as error:
            logger.error(f"Exception {error} decoding payload: {str(event.message.payload, 'utf-8)')}")
            logger.error(f"Probable cause: common messaging library supports only json data")
            return
        self._callback_func(received_topic, MessageBuilder.build(received_payload))


class GreengrassIpcProvider(MessagingProvider):

    def __init__(self, receive_own_messages: bool):
        super().__init__()
        self._ipc_subscription_handlers = {}
        self._ipc_subscription_operations = {}
        self._iot_core_subscription_handlers = {}
        self._iot_core_subscription_operations = {}
        self._response_ious = {}
        self._receive_mode = 'RECEIVE_MESSAGES_FROM_OTHERS'
        if receive_own_messages:
            self._receive_mode = 'RECEIVE_ALL_MESSAGES'
        self._ipc_client = GreengrassCoreIPCClientV2()

    def publish(self, topic: str, msg: Message):
        msg_str = msg.dumps()
        self._ipc_client.publish_to_topic(topic=topic,
                                          publish_message=PublishMessage(binary_message=BinaryMessage(message=msg_str)))

    def publish_to_iot_core(self, topic: str, msg: Message, qos: str):
        payload = msg.dumps()
        self._ipc_client.publish_to_iot_core(topic_name=topic, payload=payload, qos=qos)

    def subscribe(self, topic_filter: str, callback: Callable[[str, Message], None]):
        logger.info(f"Subscribing to IPC messages on topic {topic_filter}")
        handler = IpcSubscriptionHandler(topic_filter, callback)
        try:
            _, operation = self._ipc_client.subscribe_to_topic(topic=topic_filter,
                                                               receive_mode=self._receive_mode,
                                                               on_stream_event=handler.on_stream_event,
                                                               on_stream_error=handler.on_stream_error,
                                                               on_stream_closed=handler.on_stream_closed)
            self._ipc_subscription_operations[topic_filter] = operation
            self._ipc_subscription_handlers[topic_filter] = handler
            logger.debug(f"Successfully subscribed to the topic filter: {topic_filter} on IPC channel")
        except UnauthorizedError:
            logger.error(f"Unauthorized error while subscribing to topic filter {topic_filter} on IPC. "
                         f"Ensure access control policy is "
                         f"defined in the component configuration")
        except (ValueError, Exception) as error:
            logger.error(f"Unable to subscribe to IPC topic filter ({topic_filter}): {error}")

    def subscribe_to_iot_core(self, topic_filter: str, callback: Callable[[str, Message], None], qos: str):
        logger.info(f"Subscribing to iot core messages on topic {topic_filter}")
        handler = IoTCoreSubscriptionHandler(topic_filter, callback)
        try:
            _, operation = self._ipc_client.subscribe_to_iot_core(topic_name=topic_filter,
                                                                  qos=qos,
                                                                  on_stream_event=handler.on_stream_event,
                                                                  on_stream_error=handler.on_stream_error,
                                                                  on_stream_closed=handler.on_stream_closed)
            self._iot_core_subscription_operations[topic_filter] = operation
            self._iot_core_subscription_handlers[topic_filter] = handler
            logger.debug(f"Successfully subscribed to the topic filter: {topic_filter} on IPC channel")
        except UnauthorizedError:
            logger.error(f"Unauthorized error while subscribing to topic filter {topic_filter} on IoT Core. "
                         f"Ensure access control policy is "
                         f"defined in the component configuration")
        except (ValueError, Exception) as error:
            logger.error(f"Unable to subscribe to IoT Core topic filter ({topic_filter}): {error}")

    def unsubscribe(self, topic_filter: str):
        if topic_filter in self._ipc_subscription_operations:
            self._ipc_subscription_operations[topic_filter].close()
            del self._ipc_subscription_operations[topic_filter]
            del self._ipc_subscription_handlers[topic_filter]
        else:
            logger.warning(f"Attempt to unsubscribe from unknown IPC topic {topic_filter}")

    def unsubscribe_from_iot_core(self, topic_filter: str):
        if topic_filter in self._iot_core_subscription_operations:
            self._iot_core_subscription_operations[topic_filter].close()
            del self._iot_core_subscription_operations[topic_filter]
            del self._iot_core_subscription_handlers[topic_filter]
        else:
            logger.warning(f"Attempt to unsubscribe from unknown IoT Core topic {topic_filter}")

    def request(self, topic: str, msg: Message) -> Iou:
        reply_to = msg.make_request()
        iou = Iou(reply_to)
        self._response_ious[reply_to] = iou
        self.subscribe(reply_to, self._on_reply_received)
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

