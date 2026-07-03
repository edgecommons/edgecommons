import logging
import time
from abc import ABC

from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)

logger = logging.getLogger("<<COMPONENTNAME>>")


class <<COMPONENTNAME>>(ConfigurationChangeListener, ABC):
    def __init__(self, gg):
        super().__init__()
        self._gg = gg
        self._config_manager = gg.get_config_manager()
        self._config_manager.add_config_change_listener(self)

    def on_configuration_change(self, configuration) -> bool:
        logger.info("Configuration changed.  Ignoring.")
        return True

    def run(self):
        # Mint any topic you publish or subscribe through the UNS topic builder — never
        # hand-write one. Topics carry the component's config-resolved identity
        # (ecv1/{device}/{component}/{instance}/{class}/...), e.g.:
        #
        #   from ggcommons.uns import UnsClass
        #   topic = self._gg.uns().topic(UnsClass.APP, "my-channel")
        #   self._gg.get_messaging().publish(topic, message)
        #
        # The library already publishes the `state` heartbeat keepalive for you.
        while True:
            logger.info("Running...")
            time.sleep(10)
