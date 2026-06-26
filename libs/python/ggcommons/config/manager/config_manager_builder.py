import logging
import os
from argparse import Namespace
from typing import List

from ggcommons.config.manager.config_component_manager import ConfigComponentManager
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.configmap_config_manager import ConfigMapConfigManager
from ggcommons.config.manager.environment_config_manager import EnvironmentConfigManager
from ggcommons.config.manager.file_config_manager import FileConfigManager
from ggcommons.config.manager.greengrass_config_manager import GreengrassConfigManager
from ggcommons.config.manager.shadow_config_manager import (
    ShadowConfigManager,
    _sanitize_shadow_name,
)
from ggcommons.platform.resolver import resolve_identity

logger = logging.getLogger("ConfigManagerBuilder")


class ConfigManagerBuilder:
    @staticmethod
    def build(args: Namespace, component_name: str) -> ConfigManager:
        config_args = args.config
        # Use the identity already resolved by the platform resolver (canonical
        # precedence: -t > AWS_IOT_THING_NAME > NOT_GREENGRASS). Fall back to the
        # resolver for callers that construct args without going through it.
        thing_name = getattr(args, "identity", None)
        if thing_name is None:
            thing_name = resolve_identity(getattr(args, "thing", None), None, os.environ)
        if config_args[0].upper() == "FILE":
            logger.info("Config file specified. Using FileConfigManager")
            config_file = config_args[1] if len(config_args) > 1 else "config.json"
            config_manager = FileConfigManager(thing_name, component_name, config_file)
        elif config_args[0].upper() == "CONFIGMAP":
            logger.info("CONFIGMAP specified. Using ConfigMapConfigManager")
            # -c CONFIGMAP [mount_dir] [key]; defaults applied inside the manager
            # (/etc/ggcommons, config.json). The k8s-native source; default on KUBERNETES.
            mount_dir = config_args[1] if len(config_args) > 1 else None
            config_key = config_args[2] if len(config_args) > 2 else None
            config_manager = ConfigMapConfigManager(
                thing_name, component_name, mount_dir, config_key
            )
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
            # Explicit name verbatim; the component-name default is sanitized to a
            # valid AWS IoT shadow name ([A-Za-z0-9:_-]) — component names contain
            # dots, which AWS shadow names reject. Mirrors Java/Rust/TS.
            shadow_name = (
                config_args[1] if len(config_args) > 1 else _sanitize_shadow_name(component_name)
            )
            config_manager = ShadowConfigManager(
                thing_name, component_name, shadow_name
            )
        elif config_args[0].upper() == "CONFIG_COMPONENT":
            logger.info("CONFIG_COMPONENT specified. Using ConfigComponentManager")
            config_manager = ConfigComponentManager(thing_name, component_name)
        else:
            logger.fatal(
                f"Unrecognized config source '{config_args[0]}'.  "
                f"Valid values are 'FILE', 'CONFIGMAP', 'ENV', 'SHADOW', 'GG_CONFIG' and 'CONFIG_COMPONENT' "
            )
            raise ValueError(
                f"Unrecognized config source '{config_args[0]}'. Valid values are "
                f"'FILE', 'CONFIGMAP', 'ENV', 'SHADOW', 'GG_CONFIG' and 'CONFIG_COMPONENT'"
            )
        return config_manager
