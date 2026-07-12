import logging

from awsiot.greengrasscoreipc.clientv2 import GreengrassCoreIPCClientV2

from edgecommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("GreengrassConfigManager")


class GreengrassConfigManager(ConfigManager):
    def __init__(
        self,
        thing_name: str,
        component_name: str,
        config_component_name: str,
        config_key: str,
        platform=None,
        candidate_validators=None,
        validation_timeout_secs=5.0,
    ):
        super().__init__(
            component_name,
            thing_name,
            platform=platform,
            candidate_validators=candidate_validators,
            validation_timeout_secs=validation_timeout_secs,
        )
        self._config_component_name = config_component_name
        self._config_key = config_key if config_key is not None else "ComponentConfig"
        self._config_source = f"Greengrass config (component: {config_component_name}; key: {self._config_key})"
        self._config_provider_family = "GG_CONFIG"
        self.init()

    def _load_configuration(self) -> dict:
        logger.info(
            f"Loading Greengrass component configuration from component '{self._config_component_name}'"
        )
        ipc_client = GreengrassCoreIPCClientV2()
        if self._config_component_name is None:
            response = ipc_client.get_configuration()
        else:
            response = ipc_client.get_configuration(
                component_name=self._config_component_name
            )
        logger.debug(f"Full configuration retrieved from Nucleus: {response.value}")
        ret_val = None
        if response.value is not None:
            if self._config_key in response.value:
                ret_val = response.value.get(self._config_key)
                logger.debug(f"Component configuration retrieved: {ret_val}")
            else:
                ipc_client.close()
                raise RuntimeError(
                    f"Configuration not found in component '{self._config_component_name}' at key '{self._config_key}'"
                )
        ipc_client.close()
        return ret_val

