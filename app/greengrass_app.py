import logging
import time
from abc import ABC
from argparse import Namespace

from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import Message, MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient

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

    def hello_world_handler(self, topic: str, msg: Message):
        logger.info(f"Received a hello world message on topic {topic}: {msg.dumps()}")

    def run(self):
        i = 1
        try:
            MessagingClient.subscribe("test/hello_world", self.hello_world_handler)
            while True:
                test_message = MessageBuilder.build_from_config(name="hello_world",
                                                                version="1.0.0",
                                                                payload={"message_num" : i, "hello": "world!"},
                                                                config_manager=self._config_manager)
                # logger.info(f"Publishing message {test_message.dumps()}")
                MessagingClient.publish("test/hello_world", test_message)
                i += 1
                time.sleep(self._publish_interval)
        except KeyboardInterrupt:
            print("Finished")

    def on_configuration_change(self, configuration) -> bool:
        self._publish_interval = self._config_manager.get_global_config()['publish_interval']
        return True