import json
import logging
import os

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.split_config import BaseLayer, resolve_env_base

logger = logging.getLogger("EnvironmentConfigManager")


class EnvironmentConfigManager(ConfigManager):
    def __init__(
        self,
        thing_name: str,
        component_name: str,
        environment_variable_name: str,
        platform=None,
        no_shared_config: bool = False,
    ):
        self._environment_variable_name = environment_variable_name
        super().__init__(
            component_name, thing_name, platform=platform, no_shared_config=no_shared_config
        )
        self._config_source = f"Environment (var name: {environment_variable_name})"
        self._config_provider_family = "ENV"
        self.init()

    def _load_configuration(self) -> dict:
        if self._environment_variable_name not in os.environ:
            logger.fatal(
                f"Expecting Greengrass component configuration in '{self._environment_variable_name}' environment variable. "
                f"Check component recipe to ensure '{self._environment_variable_name}' environment variable is set to the "
                f"component configuration in the 'Run' lifecycle section."
            )
            raise RuntimeError(
                f"Configuration environment variable '{self._environment_variable_name}' is not set"
            )
        return json.loads(os.environ.get(self._environment_variable_name))

    def _resolve_base_layer(self, component_layer: dict) -> BaseLayer:
        return resolve_env_base()
