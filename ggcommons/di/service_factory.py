"""
Service factory for creating and registering default service implementations.

This module provides factory methods for creating concrete service implementations
and registering them in the service registry.
"""

from typing import TYPE_CHECKING
from ggcommons.interfaces import IConfigurationService, IMessagingService, IMetricService

if TYPE_CHECKING:
    from ggcommons.di.service_registry import ServiceRegistry
    from ggcommons.config.manager.config_manager import ConfigManager


class ServiceFactory:
    """
    Factory for creating and registering default service implementations.
    """

    @staticmethod
    def register_default_services(registry: 'ServiceRegistry', config_manager: 'ConfigManager') -> None:
        """
        Registers default service implementations in the service registry.
        
        Args:
            registry: The service registry to register services in
            config_manager: The configuration manager instance
            
        Raises:
            ValueError: If registry or config_manager is None
        """
        if registry is None:
            raise ValueError("Registry cannot be None")
        if config_manager is None:
            raise ValueError("Config manager cannot be None")

        # Import here to avoid circular imports
        from ggcommons.config.configuration_service import ConfigurationService
        from ggcommons.messaging.messaging_service import MessagingService
        from ggcommons.metrics.metric_service import MetricService

        # Register configuration service
        config_service = ConfigurationService(config_manager)
        registry.register(IConfigurationService, config_service)

        # Register messaging service
        messaging_service = MessagingService()
        registry.register(IMessagingService, messaging_service)

        # Register metric service
        metric_service = MetricService(config_manager)
        registry.register(IMetricService, metric_service)

    @staticmethod
    def create_configuration_service(config_manager: 'ConfigManager') -> IConfigurationService:
        """
        Creates a configuration service instance.
        
        Args:
            config_manager: The configuration manager instance
            
        Returns:
            Configuration service implementation
            
        Raises:
            ValueError: If config_manager is None
        """
        if config_manager is None:
            raise ValueError("Config manager cannot be None")
            
        from ggcommons.config.configuration_service import ConfigurationService
        return ConfigurationService(config_manager)

    @staticmethod
    def create_messaging_service() -> IMessagingService:
        """
        Creates a messaging service instance.
        
        Returns:
            Messaging service implementation
        """
        from ggcommons.messaging.messaging_service import MessagingService
        return MessagingService()

    @staticmethod
    def create_metric_service(config_manager: 'ConfigManager') -> IMetricService:
        """
        Creates a metric service instance.
        
        Args:
            config_manager: The configuration manager instance
            
        Returns:
            Metric service implementation
            
        Raises:
            ValueError: If config_manager is None
        """
        if config_manager is None:
            raise ValueError("Config manager cannot be None")
            
        from ggcommons.metrics.metric_service import MetricService
        return MetricService(config_manager)