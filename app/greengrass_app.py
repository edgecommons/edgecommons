import logging
import time
from abc import ABC
from argparse import Namespace

from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("<<COMPONENTNAME>>")


class <<COMPONENTNAME>>(ConfigurationChangeListener, ABC):
    def __init__(self, args: Namespace, config_manager: ConfigManager):
        super().__init__()
        self._config_manager = config_manager

    def on_configuration_change(self, configuration) -> bool:
        logger.info("Configuration changed.  Ignoring.")
        return True

    def run(self):
        while True:
            logger.info("Running...")
            time.sleep(10)
