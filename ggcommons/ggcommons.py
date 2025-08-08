"""
Enhanced GGCommons main class with dependency injection and builder support.

This module provides the main GGCommons class with enhanced features including
dependency injection, service registry, and builder pattern support.
"""

import argparse
import logging
from typing import Optional, List, TypeVar, Type
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.config_manager_builder import ConfigManagerBuilder
from ggcommons.di.service_registry import ServiceRegistry
from ggcommons.di.service_factory import ServiceFactory
from ggcommons.interfaces import IConfigurationService, IMessagingService, IMetricService

T = TypeVar('T')
logger = logging.getLogger(__name__)


class GGCommons:
    """
    Main entry point for the GGCommons framework with enhanced features.
    
    Provides dependency injection, service registry, and improved initialization.
    """

    def __init__(self, component_name: str, args: List[str], 
                 app_options: Optional[argparse.ArgumentParser] = None,
                 receive_own_messages: bool = True):
        """
        Initialize GGCommons with enhanced features.
        
        Args:
            component_name: The fully qualified component name
            args: Command line arguments
            app_options: Optional custom argument parser
            receive_own_messages: Whether to receive own messages (IPC only)
            
        Raises:
            ValueError: If component_name is None or empty
        """
        if not component_name:
            raise ValueError("Component name cannot be None or empty")
            
        self._component_name = component_name
        self._config_manager: Optional[ConfigManager] = None
        self._service_registry: Optional[ServiceRegistry] = None
        
        try:
            # Process command line arguments
            parsed_args = self._process_args(component_name, args, app_options)
            
            # Initialize configuration manager
            self._init_config_manager(component_name, parsed_args)
            
            # Initialize service registry
            self._init_service_registry()
            
            # Initialize messaging client
            self._init_messaging(parsed_args, receive_own_messages)
            
            # Initialize metric emitter
            self._init_metrics()
            
            # Initialize heartbeat
            self._init_heartbeat()
            
            # Complete initialization
            if hasattr(self._config_manager, 'complete_initialization'):
                self._config_manager.complete_initialization()
                
            logger.info("GGCommons initialized successfully")
            
        except Exception as e:
            logger.error(f"Failed to initialize GGCommons: {e}")
            raise
            
    def _process_args(self, component_name: str, args: List[str], 
                     app_options: Optional[argparse.ArgumentParser]) -> argparse.Namespace:
        """
        Process command line arguments.
        
        Args:
            component_name: The component name
            args: Command line arguments
            app_options: Optional custom argument parser
            
        Returns:
            Parsed arguments namespace
        """
        parser = app_options or argparse.ArgumentParser()
        
        # Add standard ggcommons arguments
        parser.add_argument(
            '-c', '--config',
            nargs='*',
            type=str,
            default=['GG_CONFIG'],
            help='Configuration source. One of: ENV, GG_CONFIG, FILE, SHADOW, CONFIG_COMPONENT'
        )
        parser.add_argument(
            '-m', '--mode',
            nargs='*',
            type=str,
            help='Runtime mode - GREENGRASS (default) or STANDALONE <config_file_path>'
        )
        parser.add_argument(
            '-t', '--thing',
            type=str,
            help='Thing name to use (optional)'
        )
        
        parsed = parser.parse_args(args)
        
        # Process mode argument to match Java behavior
        if not hasattr(parsed, 'mode') or not parsed.mode:
            parsed.mode = ['GREENGRASS']
        
        # Validate STANDALONE mode has config path
        if parsed.mode[0].upper() == 'STANDALONE':
            if len(parsed.mode) < 2:
                logger.error("STANDALONE mode requires config file path")
                raise ValueError("STANDALONE mode requires config file path")
        
        return parsed
        
    def _init_config_manager(self, component_name: str, parsed_args: argparse.Namespace) -> None:
        """
        Initialize the configuration manager.
        
        Args:
            component_name: The component name
            parsed_args: Parsed command line arguments
        """
        # Use config manager builder to create appropriate manager
        self._config_manager = ConfigManagerBuilder.build(parsed_args, component_name)
        
    def _init_service_registry(self) -> None:
        """Initialize the service registry with default services."""
        self._service_registry = ServiceRegistry()
        ServiceFactory.register_default_services(self._service_registry, self._config_manager)
        
    def _init_messaging(self, parsed_args: argparse.Namespace, receive_own_messages: bool) -> None:
        """
        Initialize the messaging client.
        
        Args:
            parsed_args: Parsed command line arguments
            receive_own_messages: Whether to receive own messages
        """
        # Import here to avoid circular imports
        from ggcommons.messaging.messaging_client import MessagingClient
        
        # Determine standalone config path
        standalone_config_path = None
        if hasattr(parsed_args, 'mode') and parsed_args.mode:
            if len(parsed_args.mode) > 1 and parsed_args.mode[0].upper() == 'STANDALONE':
                standalone_config_path = parsed_args.mode[1]
        
        MessagingClient.init(parsed_args, standalone_config_path, receive_own_messages)
        
    def _init_metrics(self) -> None:
        """Initialize the metric emitter."""
        # Import here to avoid circular imports
        from ggcommons.metrics.metric_emitter import MetricEmitter
        
        # Inject messaging service if available
        messaging_service = self.get_service(IMessagingService)
        if messaging_service and hasattr(MetricEmitter, 'set_messaging_service'):
            MetricEmitter.set_messaging_service(messaging_service)
            
        MetricEmitter.init(self._config_manager)
        
    def _init_heartbeat(self) -> None:
        """Initialize the heartbeat system."""
        # Import here to avoid circular imports
        from ggcommons.heartbeat.heartbeat import Heartbeat
        
        config_service = self.get_service(IConfigurationService)
        heartbeat = Heartbeat(config_service)
        
        # Inject services if available
        messaging_service = self.get_service(IMessagingService)
        metric_service = self.get_service(IMetricService)
        
        if hasattr(heartbeat, 'set_messaging_service') and messaging_service:
            heartbeat.set_messaging_service(messaging_service)
        if hasattr(heartbeat, 'set_metric_service') and metric_service:
            heartbeat.set_metric_service(metric_service)
            
    def get_config_manager(self) -> ConfigManager:
        """
        Get the configuration manager instance.
        
        Returns:
            The configuration manager
            
        Raises:
            RuntimeError: If not properly initialized
        """
        if self._config_manager is None:
            raise RuntimeError("GGCommons not properly initialized")
        return self._config_manager
        
    def get_configuration_service(self) -> IConfigurationService:
        """
        Get the configuration service interface.
        
        Returns:
            The configuration service
        """
        return self.get_service(IConfigurationService)
        
    def get_service(self, service_type: Type[T]) -> Optional[T]:
        """
        Retrieve a service by its interface type.
        
        Args:
            service_type: The service interface class
            
        Returns:
            The service implementation, or None if not registered
            
        Raises:
            RuntimeError: If service registry not initialized
            ValueError: If service_type is None
        """
        if service_type is None:
            raise ValueError("Service type cannot be None")
        if self._service_registry is None:
            raise RuntimeError("GGCommons not properly initialized")
            
        return self._service_registry.get(service_type)
        
    def register_service(self, service_type: Type[T], implementation: T) -> None:
        """
        Register a custom service implementation.
        
        Args:
            service_type: The service interface class
            implementation: The service implementation
            
        Raises:
            RuntimeError: If service registry not initialized
            ValueError: If parameters are invalid
        """
        if service_type is None:
            raise ValueError("Service type cannot be None")
        if implementation is None:
            raise ValueError("Implementation cannot be None")
        if self._service_registry is None:
            raise RuntimeError("GGCommons not properly initialized")
            
        self._service_registry.register(service_type, implementation)
        
    def get_service_registry(self) -> ServiceRegistry:
        """
        Get the service registry for advanced service management.
        
        Returns:
            The service registry instance
            
        Raises:
            RuntimeError: If not properly initialized
        """
        if self._service_registry is None:
            raise RuntimeError("GGCommons not properly initialized")
        return self._service_registry
        
    def shutdown(self) -> None:
        """
        Shutdown GGCommons and clean up resources.
        """
        try:
            # Shutdown messaging client
            from ggcommons.messaging.messaging_client import MessagingClient
            MessagingClient.shutdown()
            
            # Shutdown heartbeat if available
            from ggcommons.heartbeat.heartbeat import Heartbeat
            if hasattr(Heartbeat, 'shutdown'):
                Heartbeat.shutdown()
                
            # Clear service registry
            if self._service_registry:
                self._service_registry.clear()
                
            logger.info("GGCommons shutdown completed")
            
        except Exception as e:
            logger.error(f"Error during GGCommons shutdown: {e}")