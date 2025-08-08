import logging
import os
import uuid
from argparse import Namespace
from typing import List

from ggcommons.config.manager.config_component_manager import ConfigComponentManager
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.environment_config_manager import EnvironmentConfigManager
from ggcommons.config.manager.file_config_manager import FileConfigManager
from ggcommons.config.manager.greengrass_config_manager import GreengrassConfigManager
from ggcommons.config.manager.shadow_config_manager import ShadowConfigManager

logger = logging.getLogger("ConfigManagerBuilder")


class ConfigManagerBuilder:
    @staticmethod
    def build(args: Namespace, component_name: str) -> ConfigManager:
        config_args = args.config
        if "AWS_IOT_THING_NAME" in os.environ:
            thing_name = os.environ["AWS_IOT_THING_NAME"]
        elif args.thing is not None:
            thing_name = args.thing
        else:
            thing_name = str(uuid.uuid4())
        if config_args[0].upper() == "FILE":
            logger.info("Config file specified. Using FileConfigManager")
            config_file = config_args[1] if len(config_args) > 1 else "config.json"
            config_manager = FileConfigManager(thing_name, component_name, config_file)
        elif config_args[0].upper() == "ENV":
            logger.info("Environment config specified. Using EnvironmentConfigManager")
            env_var = config_args[1] if len(config_args) > 1 else "CONFIG"
            config_manager = EnvironmentConfigManager(
                thing_name, component_name, env_var
            )
        elif config_args[0].upper() == "GG_CONFIG":
            logger.info("GG_CONFIG specified. Using GreengrassConfigManager")
            config_component_name = config_args[1] if len(config_args) > 1 else None
            config_component_key = config_args[2] if len(config_args) > 2 else None
            config_manager = GreengrassConfigManager(
                thing_name, component_name, config_component_name, config_component_key
            )
        elif config_args[0].upper() == "SHADOW":
            logger.info("SHADOW specified. Using ShadowConfigManager")
            shadow_name = config_args[1] if len(config_args) > 1 else component_name
            config_manager = ShadowConfigManager(
                thing_name, component_name, shadow_name
            )
        elif config_args[0].upper() == "CONFIG_COMPONENT":
            logger.info("CONFIG_COMPONENT specified. Using ConfigComponentManager")
            config_manager = ConfigComponentManager(thing_name, component_name)
        else:
            logger.fatal(
                f"Unrecognized config source '{config_args[0]}'.  "
                f"Valid values are 'FILE', 'ENV', 'SHADOW', 'GG_CONFIG' and 'CONFIG_COMPONENT' "
            )
            exit(5)
        return config_manager
