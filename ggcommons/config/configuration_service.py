"""
Configuration service implementation.

This module provides a concrete implementation of IConfigurationService
that wraps the existing ConfigManager functionality.
"""

from typing import Collection, Dict, Any, Optional
from ggcommons.interfaces.i_configuration_service import IConfigurationService
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.config.manager.config_manager import ConfigManager


class ConfigurationService(IConfigurationService):
    """
    Service implementation that wraps ConfigManager to provide the IConfigurationService interface.
    This allows for dependency injection while maintaining backward compatibility.
    """

    def __init__(self, config_manager: ConfigManager):
        """
        Initialize the configuration service with a config manager.
        
        Args:
            config_manager: The underlying configuration manager
            
        Raises:
            ValueError: If config_manager is None
        """
        if config_manager is None:
            raise ValueError("Config manager cannot be None")
        self._config_manager = config_manager

    def get_global_config(self) -> Dict[str, Any]:
        """
        Returns the global configuration section shared across all instances.
        
        Returns:
            Dict containing global configuration settings
        """
        global_config = self._config_manager.get_global_config()
        # Convert JsonObject to dict if needed
        if hasattr(global_config, 'to_dict'):
            return global_config.to_dict()
        return dict(global_config) if global_config else {}

    def get_instance_config(self, instance_id: str) -> Optional[Dict[str, Any]]:
        """
        Returns configuration for a specific instance.
        
        Args:
            instance_id: The instance identifier
            
        Returns:
            Dict containing instance-specific configuration, or None if not found
            
        Raises:
            ValueError: If instance_id is None or empty
        """
        if not instance_id:
            raise ValueError("Instance ID cannot be None or empty")
            
        instance_config = self._config_manager.get_instance_config(instance_id)
        if instance_config is None:
            return None
            
        # Convert JsonObject to dict if needed
        if hasattr(instance_config, 'to_dict'):
            return instance_config.to_dict()
        return dict(instance_config)

    def get_instance_ids(self) -> Collection[str]:
        """
        Returns collection of all configured instance IDs.
        
        Returns:
            Collection of instance identifier strings
        """
        return self._config_manager.get_instance_ids()

    def get_full_config(self) -> Dict[str, Any]:
        """
        Returns the complete configuration object.
        
        Returns:
            Dict containing the full configuration
        """
        full_config = self._config_manager.get_full_config()
        # Convert JsonObject to dict if needed
        if hasattr(full_config, 'to_dict'):
            return full_config.to_dict()
        return dict(full_config) if full_config else {}

    def get_thing_name(self) -> Optional[str]:
        """
        Returns the AWS IoT Thing name.
        
        Returns:
            The thing name or None if not available
        """
        return self._config_manager.get_thing_name()

    def get_component_name(self) -> str:
        """
        Returns the short component name.
        
        Returns:
            The component name
        """
        return self._config_manager.get_component_name()

    def get_component_full_name(self) -> str:
        """
        Returns the fully qualified component name.
        
        Returns:
            The fully qualified component name
        """
        return self._config_manager.get_component_full_name()

    def resolve_template(self, template: str) -> str:
        """
        Resolves template variables in a string.
        
        Args:
            template: String containing template variables like {ThingName}
            
        Returns:
            Resolved string with substituted values
            
        Raises:
            ValueError: If template is None
        """
        if template is None:
            raise ValueError("Template cannot be None")
        return self._config_manager.resolve_template(template)

    def add_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """
        Registers a configuration change listener.
        
        Args:
            listener: The listener to add
            
        Raises:
            ValueError: If listener is None
        """
        if listener is None:
            raise ValueError("Listener cannot be None")
        self._config_manager.add_config_change_listener(listener)

    def remove_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """
        Removes a configuration change listener.
        
        Args:
            listener: The listener to remove
            
        Raises:
            ValueError: If listener is None
        """
        if listener is None:
            raise ValueError("Listener cannot be None")
        self._config_manager.remove_config_change_listener(listener)

    def notify_configuration_changed(self) -> None:
        """
        Manually triggers configuration change notifications.
        """
        self._config_manager.notify_configuration_changed()

    def get_tag_config(self):
        """
        Returns the tag configuration object.
        
        Returns:
            TagConfiguration object or None if not available
        """
        return self._config_manager.get_tag_config()

    def get_heartbeat_config(self):
        """
        Returns the heartbeat configuration object.
        
        Returns:
            HeartbeatConfiguration object
        """
        return self._config_manager.get_heartbeat_config()

    def get_metric_config(self):
        """
        Returns the metric configuration object.
        
        Returns:
            MetricConfiguration object
        """
        return self._config_manager.get_metric_config()

    def get_logging_config(self):
        """
        Returns the logging configuration object.
        
        Returns:
            LoggingConfiguration object
        """
        return self._config_manager.get_logging_config()

    def get_config_source(self) -> str:
        """
        Returns the configuration source identifier.
        
        Returns:
            String identifying the configuration source
        """
        return self._config_manager.get_config_source()

    def is_validation_enabled(self) -> bool:
        """
        Returns whether configuration validation is enabled.
        
        Returns:
            True if validation is enabled, False otherwise
        """
        return self._config_manager.is_validation_enabled()

    def is_initializing(self) -> bool:
        """
        Returns whether the configuration manager is still initializing.
        
        Returns:
            True if still initializing, False otherwise
        """
        return self._config_manager.is_initializing()

    @property
    def config_manager(self) -> ConfigManager:
        """
        Returns the underlying config manager for backward compatibility.
        
        Returns:
            The wrapped ConfigManager instance
        """
        return self._config_manager