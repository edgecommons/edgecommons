import logging
import time
from abc import ABC

from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("<<COMPONENTNAME>>")


class <<COMPONENTNAME>>(ConfigurationChangeListener, ABC):
    def __init__(self, config_manager: ConfigManager):
        super().__init__()
        self._config_manager = config_manager
        self._config_manager.add_config_change_listener(self)

    def on_configuration_change(self, configuration) -> bool:
        logger.info("Configuration changed.  Ignoring.")
        return True

    def run(self):
        while True:
            logger.info("Running...")
            time.sleep(10)
