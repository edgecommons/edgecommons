import logging
import time
from abc import ABC
from argparse import Namespace
from awsiot.greengrasscoreipc.model import QOS
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import Message, MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.utils.iou import Iou

logger = logging.getLogger("GreengrassApp")


# This sample application subscribes to messages on the topic "hello/world" and
# then publishes a message every n seconds on that topic, where "n" comes from the
# app specific configuration section in the config file/recipe.  The message is output
# to the log.  The application inherits configuration management, heartbeats, logging
# and switching between local MQTT and GG IPC from ggcommons.


class GreengrassApp(ConfigurationChangeListener, ABC):

    def __init__(self, args: Namespace, config_manager: ConfigManager):
        super().__init__()
        self._config_manager = config_manager
        self._config_manager.add_config_change_listener(self)
        global_config = self._config_manager.get_global_config()
        self._publish_interval = global_config['publish_interval'] if 'publish_interval' in global_config else 5

    def ipc_hello_world_handler(self, topic: str, msg: Message):
        logger.info(f"Received an ipc hello world message on topic {topic}: {msg.get_body()['id']}")

    def iot_core_hello_world_handler(self, topic: str, msg: Message):
        logger.info(f"Received an iot core hello world message on topic {topic}: {msg.get_body()['id']}")

    def request_callback(self, topic: str, request: Message):
        logger.info(f"Received request message [{topic}]: {request.get_body()['id']}")
        reply_payload = {'reply_message': "I have received your request and have replied with this message"}
        reply = MessageBuilder.build_from_config("ReplyTest", "1.0", reply_payload, self._config_manager)
        time.sleep(request.get_body()['wait_time'])
        logger.info(f"Publishing reply message {request.get_body()['id']}")
        MessagingClient.reply(request, reply)

    def publish_request(self, id: str, execution_time: float) -> Iou:
        logger.info(f"Publishing reqeust message {id}")
        request_payload = {"id": id, "wait_time": execution_time}
        request = MessageBuilder.build_from_config("RequestTest", "1.0", request_payload, self._config_manager)
        return MessagingClient.request("ggcommons/test/python/request", request)

    def wait_for_reply(self, msg_instance: str, iou: Iou, timeout: float):
        logger.info(f"Waiting for reply for {msg_instance}")
        done, reply = iou.get(timeout)
        if done is False:
            logger.warning(f"Reply for {msg_instance} timed out (took more than {timeout} seconds). Cancelling.")
            MessagingClient.cancel_request(reply)
        else:
            logger.info(f"...Received reply for {msg_instance}: {reply.dumps()}")

    def run(self):
        i = 1
        try:
            MessagingClient.subscribe("ggcommons/test/python/hello_world", self.ipc_hello_world_handler)
            MessagingClient.subscribe_to_iot_core("ggcommons/test/python/hello_world", self.iot_core_hello_world_handler, QOS.AT_LEAST_ONCE)
            MessagingClient.subscribe("ggcommons/test/python/request", self.request_callback)

            iou_1 = self.publish_request(id="1", execution_time=0)
            iou_2 = self.publish_request(id="2", execution_time=1)
            iou_3 = self.publish_request(id="3", execution_time=5)
            self.wait_for_reply("iou_1", iou_1, 1)
            self.wait_for_reply("iou_3", iou_3, 3)
            self.wait_for_reply("iou_2", iou_2, 2)

            while True:
                test_message = MessageBuilder.build_from_config(name="hello_world",
                                                                version="1.0.0",
                                                                payload={"id": i, "message": "Hello World Python"},
                                                                config_manager=self._config_manager)
                logger.info(f"Publishing message {i} to ipc")
                MessagingClient.publish("ggcommons/test/python/hello_world", test_message)
                logger.info(f"Publishing message {i} to iot core")
                MessagingClient.publish_to_iot_core("ggcommons/test/python/hello_world", test_message, QOS.AT_LEAST_ONCE)
                i += 1
                time.sleep(self._publish_interval)
        except KeyboardInterrupt:
            print("Finished")

    def on_configuration_change(self, configuration) -> bool:
        self._publish_interval = self._config_manager.get_global_config()['publish_interval']
        return True
