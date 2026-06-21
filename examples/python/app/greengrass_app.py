import json
import logging
import time
from abc import ABC
from random import random

from awsiot.greengrasscoreipc.model import QOS
from ggcommons.utils.iou import Iou
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import Message
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient

logger = logging.getLogger("GreengrassApp")


# This sample application subscribes to messages on the topic "hello/world" and
# then publishes a message every n seconds on that topic, where "n" comes from the
# app specific configuration section in the config file/recipe.  The message is output
# to the log.  The application inherits configuration management, heartbeats, logging
# and switching between local MQTT and GG IPC from ggcommons.


class GreengrassApp(ConfigurationChangeListener, ABC):
    def __init__(self, config_manager: ConfigManager, streams=None):
        super().__init__()
        self._config_manager = config_manager
        self._config_manager.add_config_change_listener(self)
        global_config = self._config_manager.get_global_config()
        self._publish_interval = (
            global_config["publish_interval"]
            if "publish_interval" in global_config
            else 5
        )
        # Durable telemetry stream handle (None unless the config has a `streaming` section and
        # a stream named "telemetry"). The publish loop appends each message here; the library's
        # export engine drains it to the configured sink (Kinesis) independently.
        self._stream = None
        if streams is not None:
            try:
                self._stream = streams.stream("telemetry")
                logger.info("Telemetry streaming enabled (stream 'telemetry')")
            except Exception as e:
                logger.warning(f"stream 'telemetry' unavailable; streaming disabled: {e}")
        self.define_metric()

    def ipc_hello_world_handler(self, topic: str, msg: Message):
        logger.info(
            f"Received an ipc hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )
        time.sleep(5)
        logger.info(
            f"#### Received an ipc hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )

    def iot_core_hello_world_handler(self, topic: str, msg: Message):
        logger.info(
            f"Received an iot core hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )
        time.sleep(5)
        logger.info(
            f"Received an iot core hello world message on topic {topic}: {msg.get_body()['msg_id']}"
        )

    def request_callback(self, topic: str, request: Message):
        logger.info(f"Received request message [{topic}]: {request.get_body()['msg_id']}")
        reply_payload = {
            "reply_message": "I have received your request and have replied with this message"
        }
        reply = (
            MessageBuilder.create("ReplyTest", "1.0")
            .with_payload(reply_payload)
            .with_config(self._config_manager)
            .build()
        )
        time.sleep(request.get_body()["wait_time"])
        logger.info(f"Publishing reply message {request.get_body()['msg_id']}")
        MessagingClient.reply(request, reply)

    def publish_request(self, msg_id: str, execution_time: float) -> Iou:
        logger.info(f"Publishing reqeust message {msg_id}")
        request_payload = {"msg_id": msg_id, "wait_time": execution_time}
        request = (
            MessageBuilder.create("RequestTest", "1.0")
            .with_payload(request_payload)
            .with_config(self._config_manager)
            .build()
        )
        return MessagingClient.request("ggcommons/test/python/request", request)

    def wait_for_reply(self, msg_instance: str, iou: Iou, timeout: float):
        logger.info(f"Waiting for reply for {msg_instance}")
        done, reply = iou.get(timeout)
        if done is False:
            logger.warning(
                f"Reply for {msg_instance} timed out (took more than {timeout} seconds). Cancelling."
            )
            MessagingClient.cancel_request(reply)
        else:
            logger.info(f"...Received reply for {msg_instance}: {reply.dumps()}")

    def define_metric(self):
        metric = (
            MetricBuilder.create("performance")
            .with_config(self._config_manager)
            .add_measure("latency", "Milliseconds", 1)
            .build()
        )
        MetricEmitter.define_metric(metric)
        return metric

    def run(self):
        i = 1
        try:
            measure_values = {}
            MessagingClient.subscribe(
                "ggcommons/test/python/hello_world", self.ipc_hello_world_handler, True
            )
            # Non-fatal: builds/modes without an IoT Core transport (e.g. local-only STANDALONE)
            # skip the IoT Core bridge instead of failing the whole component.
            try:
                MessagingClient.subscribe_to_iot_core(
                    "ggcommons/test/python/hello_world",
                    self.iot_core_hello_world_handler,
                    QOS.AT_LEAST_ONCE,
                )
            except Exception as e:
                logger.warning(f"IoT Core unavailable; skipping IoT Core subscribe: {e}")
            MessagingClient.subscribe(
                "ggcommons/test/python/request", self.request_callback
            )

            iou_1 = self.publish_request(msg_id="1", execution_time=0)
            iou_2 = self.publish_request(msg_id="2", execution_time=1)
            iou_3 = self.publish_request(msg_id="3", execution_time=5)

            self.wait_for_reply("iou_1", iou_1, 1)
            self.wait_for_reply("iou_3", iou_3, 3)
            self.wait_for_reply("iou_2", iou_2, 2)

            while True:
                test_message = (
                    MessageBuilder.create("hello_world", "1.0.0")
                    .with_payload({"msg_id": i, "message": "Hello World Python"})
                    .with_config(self._config_manager)
                    .build()
                )
                logger.info(f"Publishing message {i} to ipc")
                MessagingClient.publish(
                    "ggcommons/test/python/hello_world", test_message
                )
                logger.info(f"Publishing message {i} to iot core")
                try:
                    MessagingClient.publish_to_iot_core(
                        "ggcommons/test/python/hello_world", test_message, QOS.AT_LEAST_ONCE
                    )
                except Exception as e:
                    logger.warning(f"failed to publish to IoT Core: {e}")
                # Append the data point to the durable telemetry stream (partitioned by Thing).
                if self._stream is not None:
                    thing = self._config_manager.get_thing_name()
                    payload = json.dumps({"msg_id": i, "thing": thing}).encode("utf-8")
                    try:
                        self._stream.append(thing, int(time.time() * 1000), payload)
                    except Exception as e:
                        logger.warning(f"failed to append to telemetry stream: {e}")
                measure_values["replyLatency"] = random() * 100
                MetricEmitter.emit_metric("performance", measure_values)

                i += 1
                time.sleep(self._publish_interval)
        except KeyboardInterrupt:
            print("Finished")

    def on_configuration_change(self, configuration) -> bool:
        self._publish_interval = self._config_manager.get_global_config()[
            "publish_interval"
        ]
        return True
