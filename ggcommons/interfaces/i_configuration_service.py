"""
Configuration service interface for ggcommons.

This interface defines the contract for configuration management services,
providing access to component configuration and change notifications.
"""

from abc import ABC, abstractmethod
from typing import Collection, Dict, Any, Optional
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener


class IConfigurationService(ABC):
    """
    Interface for configuration management services.
    Provides access to component configuration and change notifications.
    """

    @abstractmethod
    def get_global_config(self) -> Dict[str, Any]:
        """
        Returns the global configuration section shared across all instances.
        
        Returns:
            Dict containing global configuration settings
        """
        pass

    @abstractmethod
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
        pass

    @abstractmethod
    def get_instance_ids(self) -> Collection[str]:
        """
        Returns collection of all configured instance IDs.
        
        Returns:
            Collection of instance identifier strings
        """
        pass

    @abstractmethod
    def get_full_config(self) -> Dict[str, Any]:
        """
        Returns the complete configuration object.
        
        Returns:
            Dict containing the full configuration
        """
        pass

    @abstractmethod
    def get_thing_name(self) -> Optional[str]:
        """
        Returns the AWS IoT Thing name.
        
        Returns:
            The thing name or None if not available
        """
        pass

    @abstractmethod
    def get_component_name(self) -> str:
        """
        Returns the short component name.
        
        Returns:
            The component name
        """
        pass

    @abstractmethod
    def get_component_full_name(self) -> str:
        """
        Returns the fully qualified component name.
        
        Returns:
            The fully qualified component name
        """
        pass

    @abstractmethod
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
        pass

    @abstractmethod
    def add_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """
        Registers a configuration change listener.
        
        Args:
            listener: The listener to add
            
        Raises:
            ValueError: If listener is None
        """
        pass

    @abstractmethod
    def remove_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """
        Removes a configuration change listener.
        
        Args:
            listener: The listener to remove
            
        Raises:
            ValueError: If listener is None
        """
        pass

    @abstractmethod
    def notify_configuration_changed(self) -> None:
        """
        Manually triggers configuration change notifications.
        """
        pass

    @abstractmethod
    def get_tag_config(self):
        """
        Returns the tag configuration object.
        
        Returns:
            TagConfiguration object or None if not available
        """
        pass

    @abstractmethod
    def get_heartbeat_config(self):
        """
        Returns the heartbeat configuration object.
        
        Returns:
            HeartbeatConfiguration object
        """
        pass

    @abstractmethod
    def get_metric_config(self):
        """
        Returns the metric configuration object.
        
        Returns:
            MetricConfiguration object
        """
        pass

    @abstractmethod
    def get_logging_config(self):
        """
        Returns the logging configuration object.
        
        Returns:
            LoggingConfiguration object
        """
        pass

    @abstractmethod
    def get_config_source(self) -> str:
        """
        Returns the configuration source identifier.
        
        Returns:
            String identifying the configuration source
        """
        pass

    @abstractmethod
    def is_validation_enabled(self) -> bool:
        """
        Returns whether configuration validation is enabled.
        
        Returns:
            True if validation is enabled, False otherwise
        """
        pass

    @abstractmethod
    def is_initializing(self) -> bool:
        """
        Returns whether the configuration manager is still initializing.
        
        Returns:
            True if still initializing, False otherwise
        """
        pass