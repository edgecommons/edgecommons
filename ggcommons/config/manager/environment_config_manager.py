import json
import logging
import os
from abc import ABC

from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("EnvironmentConfigManager")


class EnvironmentConfigManager(ConfigManager, ABC):
    def __init__(self, component_name: str, environment_variable_name: str):
        self._environment_variable_name = environment_variable_name
        super().__init__(component_name)
        self.init()

    def _load_configuration(self) -> dict:
        if self._environment_variable_name not in os.environ:
            logger.fatal(
                f"Expecting Greengrass component configuration in '{self._environment_variable_name}' environment variable. "
                f"Check component recipe to ensure '{self._environment_variable_name}' environment variable is set to the "
                f"component configuration in the 'Run' lifecycle section."
            )
            exit(1)
        return json.loads(os.environ.get(self._environment_variable_name))

    def get_config_source(self) -> str:
        return f"Environment (var name: {self._environment_variable_name})"
