import logging
import time
from argparse import Namespace

from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("<<COMPONENTNAME>>")


# This sample application subscribes to messages on the topic "hello/world" and
# then publishes a message every n seconds on that topic, where "n" comes from the
# app specific configuration section in the config file/recipe.  The message is output
# to the log.  The application inherits configuration management, heartbeats, logging
# and switching between local MQTT and GG IPC from ggcommons.


class <<COMPONENTNAME>>(ConfigurationChangeListener, ABC):
    def __init__(self, args: Namespace, config_manager: ConfigManager):
        super().__init__()
        self._config_manager = config_manager


    def run(self):
        while True:
            logger.info("Running...")
            time.sleep(10)
