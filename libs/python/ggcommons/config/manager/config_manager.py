import logging
import time
from typing import Dict, Any

from ggcommons.config.heartbeat_config import HeartbeatConfiguration
from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.config.tag_config import TagConfiguration
from ggcommons.config.enhanced_logging_config import EnhancedLoggingConfiguration
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener

logger = logging.getLogger("ConfigManager")


def _sanitize(value: str) -> str:
    """Neutralize characters in a substituted value that are dangerous in a file
    path or MQTT topic: path separators (``/``, ``\\``), traversal dot sequences
    (``..``), MQTT wildcards (``+``, ``#``), and control characters are each
    replaced with ``_``. Applied only to interpolated values, never to the
    surrounding template, so structural separators in the template are preserved.
    Mirrors the Rust library's ``config::template::sanitize``.
    """
    if value is None:
        return ""
    result = []
    for c in str(value):
        o = ord(c)
        if c in "/\\+#" or o < 0x20 or 0x7F <= o <= 0x9F:
            result.append("_")
        else:
            result.append(c)
    # Collapse traversal sequences (e.g. "..") that remain after separator replacement.
    return "".join(result).replace("..", "_")


class ConfigManager:
    def __init__(self, component_name: str, thing_name: str = None, validate_config: bool = True):
        if not component_name:
            raise ValueError("Component name cannot be None or empty")
            
        self._tag_config = None
        self._heartbeat_config = None
        self._metric_config = None
        self._component_config = None
        self._logging_config = None
        self._streaming_config = None
        self._credentials_config = None
        self._global_config = {}
        self._instances = {}
        self._change_listeners = []
        self._thing_name = thing_name
        self._component_full_name = component_name
        self._validate_config = validate_config
        self._initializing = True
        self._config_source = "unknown"
        
        if "." in component_name:
            self._component_name = component_name.rpartition(".")[-1]
        else:
            self._component_name = component_name

    def init(self):
        try:
            config = self._load_configuration()
            if config is None:
                config = {"component": {}}
                
            # Validate configuration if enabled
            if self._validate_config:
                self._validate_configuration(config)
                
            self._apply_config(config)
            logger.info("Configuration manager initialized successfully")
            
        except Exception as e:
            logger.error(f"Failed to initialize configuration manager: {e}")
            raise

    def _apply_config(self, config: dict):
        # Tags first: the log file path template ({ThingName}/{ComponentName}/{tag})
        # is resolved during logging setup below, so tag_config must already exist.
        tag_json = config.get("tags")
        self._tag_config = TagConfiguration(tag_json)

        logging_json = config.get("logging")
        self._logging_config = EnhancedLoggingConfiguration(logging_json)
        # configure_logging wires the console handler plus, when
        # logging.fileLogging.enabled, a size-rotated RotatingFileHandler
        # (maxFileSize / backupCount). It clears existing handlers first, so calling
        # it again on a config hot-reload reconfigures cleanly without leaking.
        self._logging_config.configure_logging(self)
        logging.Formatter.converter = time.gmtime

        heartbeat_json = config.get("heartbeat")
        self._heartbeat_config = HeartbeatConfiguration(heartbeat_json)

        metric_json = config.get("metricEmission")
        self._metric_config = MetricConfiguration(metric_json)

        self._component_config = config.get("component", {"global": {}, "instances": []})
        self._global_config = self._component_config.get("global", {})
        self._gen_instances_map()

        # Retain the raw `streaming` section verbatim (no typed parsing in Python — the native
        # ggstreamlog core owns the streaming schema). Kept so get_full_config() exposes it to
        # GGCommons._init_streaming(); without this the section is dropped and streaming never opens.
        self._streaming_config = config.get("streaming")
        # Likewise retain the raw `credentials` section so GGCommons._init_credentials() can find it.
        self._credentials_config = config.get("credentials")

    def _gen_instances_map(self):
        # Rebuild from scratch so a hot reload that removes an instance does not
        # leave a stale entry behind.
        self._instances = {}
        if "instances" in self._component_config:
            for instance in self._component_config["instances"]:
                self._instances[instance["id"]] = instance
                logger.debug(f"loaded config for {self._instances[instance['id']]}")

    def configuration_changed(self, new_config: dict) -> bool:
        try:
            logger.debug("Processing configuration change")
            
            # Validate new configuration if enabled
            if self._validate_config:
                self._validate_configuration(new_config)
                
            # Apply the new configuration
            self._apply_config(new_config)
            
            # Notify listeners only if not initializing
            if not self._initializing:
                self._notify_configuration_changed(new_config)
                
            logger.info("Configuration change processed successfully")
            return True
            
        except Exception as e:
            logger.error(f"Failed to process configuration change: {e}")
            return False

    def resolve_template(self, template: str) -> str:
        ret_val = template
        if "{ThingName}" in template:
            ret_val = ret_val.replace("{ThingName}", _sanitize(self._thing_name))
        if "{ComponentName}" in template:
            ret_val = ret_val.replace("{ComponentName}", _sanitize(self._component_name))
        if "{ComponentFullName}" in template:
            ret_val = ret_val.replace("{ComponentFullName}", _sanitize(self._component_full_name))
        tag_dict = {} if self._tag_config is None else self._tag_config.to_dict()
        for k in tag_dict.keys():
            key_template = "{" + k + "}"
            if key_template in template:
                ret_val = ret_val.replace(key_template, _sanitize(tag_dict[k]))
        return ret_val

    def _load_configuration(self) -> dict:
        """Default implementation returns empty config. Subclasses should override."""
        return {"component": {}}

    def get_global_config(self) -> dict:
        return self._global_config

    def get_instance_ids(self) -> list:
        return [*self._instances]

    def get_instance_config(self, inst_id) -> dict:
        return self._instances[inst_id]

    def get_tag_config(self) -> TagConfiguration:
        return self._tag_config

    def get_heartbeat_config(self) -> HeartbeatConfiguration:
        return self._heartbeat_config

    def get_metric_config(self) -> MetricConfiguration:
        return self._metric_config

    def get_logging_config(self) -> EnhancedLoggingConfiguration:
        return self._logging_config

    def get_thing_name(self) -> str:
        return self._thing_name

    def get_component_name(self) -> str:
        return self._component_name

    def get_component_full_name(self) -> str:
        return self._component_full_name

    def add_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        if listener is None:
            raise ValueError("Listener cannot be None")
        self._change_listeners.append(listener)
        logger.debug(f"Added configuration change listener: {listener}")

    def get_config_source(self) -> str:
        return self._config_source
        
    def _validate_configuration(self, config: Dict[str, Any]) -> None:
        """Validates configuration. Override in subclasses for specific validation."""
        try:
            from ggcommons.validation.configuration_validator import (
                ConfigurationValidator,
                ConfigurationValidationException
            )
            ConfigurationValidator.validate(config)
            logger.debug("Configuration validation passed")
            
        except ImportError:
            logger.debug("Configuration validator not available, skipping validation")
        except Exception as e:
            logger.error(f"Configuration validation failed: {e}")
            if self._initializing:
                raise
            else:
                logger.warning("Continuing with invalid configuration")
                
    def complete_initialization(self) -> None:
        """Marks initialization as complete."""
        self._initializing = False
        logger.debug("Configuration manager initialization completed")

    def close(self) -> None:
        """Release any resources held by this config manager (e.g. file watchers).

        No-op by default; subclasses such as FileConfigManager override this to
        stop their background threads.
        """
        pass
        
    def _notify_configuration_changed(self, new_config: Dict[str, Any]) -> None:
        """Notifies all registered listeners of configuration changes."""
        logger.info(f"Notifying {len(self._change_listeners)} configuration change listeners")
        
        for listener in self._change_listeners:
            try:
                if listener is not None:
                    result = listener.on_configuration_change(new_config)
                    if not result:
                        logger.warning(f"Listener {listener} returned False for configuration change")
                else:
                    logger.error("Configuration change listener is None")
                    
            except Exception as e:
                logger.error(f"Error notifying configuration change listener {listener}: {e}")
                
    def remove_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """Removes a configuration change listener."""
        if listener is None:
            raise ValueError("Listener cannot be None")
            
        if listener in self._change_listeners:
            self._change_listeners.remove(listener)
            logger.debug(f"Removed configuration change listener: {listener}")
        else:
            logger.warning(f"Attempted to remove non-existent listener: {listener}")
            
    def get_full_config(self) -> Dict[str, Any]:
        """Returns the complete configuration object."""
        full = {
            'component': self._component_config,
            'tags': self._tag_config.to_dict() if self._tag_config else {},
            'heartbeat': self._heartbeat_config.to_dict() if self._heartbeat_config else {},
            'metricEmission': self._metric_config.to_dict() if self._metric_config else {},
            'logging': self._logging_config.to_dict() if self._logging_config else {}
        }
        # Surface the raw streaming section (if any) so GGCommons._init_streaming() can find it.
        if self._streaming_config is not None:
            full['streaming'] = self._streaming_config
        if self._credentials_config is not None:
            full['credentials'] = self._credentials_config
        return full
        
    def is_validation_enabled(self) -> bool:
        """Returns whether configuration validation is enabled."""
        return self._validate_config
        
    def is_initializing(self) -> bool:
        """Returns whether the configuration manager is still initializing."""
        return self._initializing
