import logging
from typing import List

from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.environment_config_manager import EnvironmentConfigManager
from ggcommons.config.manager.file_config_manager import FileConfigManager
from ggcommons.config.manager.greengrass_config_manager import GreengrassConfigManager
from ggcommons.config.manager.shadow_config_manager import ShadowConfigManager

logger = logging.getLogger("ConfigManagerBuilder")

class ConfigManagerBuilder:

    @staticmethod
    def build(config_args: List[str], component_name: str) -> ConfigManager:
        if config_args[0].upper() == "FILE":
            logger.info("Config file specified. Using FileConfigManager")
            config_file = config_args[1] if len(config_args) > 1 else "config.json"
            config_manager = FileConfigManager(component_name, config_file)
        elif config_args[0].upper() == "ENV":
            logger.info("Environment config specified. Using EnvironmentConfigManager")
            env_var = config_args[1] if len(config_args) > 1 else "CONFIG"
            config_manager = EnvironmentConfigManager(component_name, env_var)
        elif config_args[0].upper() == "GG_CONFIG":
            logger.info("GG_CONFIG specified. Using GreengrassConfigManager")
            config_component_name = config_args[1] if len(config_args) > 1 else None
            config_component_key = config_args[2] if len(config_args) > 2 else None
            config_manager = GreengrassConfigManager(component_name, config_component_name, config_component_key)
        elif config_args[0].upper() == "SHADOW":
            logger.info("SHADOW specified. Using ShadowConfigManager")
            shadow_name = config_args[1] if len(config_args) > 1 else component_name
            config_manager = ShadowConfigManager(component_name, shadow_name)
        else:
            logger.fatal(f"Unrecognized config source '{config_args[0]}'.  Valid values are 'FILE', 'ENV', 'SHADOW' and 'GG_CONFIG")
            exit(5)
        return config_manager
