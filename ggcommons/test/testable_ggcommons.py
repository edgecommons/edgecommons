"""
Testable GGCommons implementation for unit and integration testing.

This module provides a testable version of GGCommons that allows service injection
before initialization, enabling true unit testing with mock services.
"""

import logging
from typing import List, Optional, Dict, Any
from ggcommons.ggcommons import GGCommons
from ggcommons.di.service_registry import ServiceRegistry
from ggcommons.interfaces import IConfigurationService, IMessagingService, IMetricService
from ggcommons.test.mock_services import MockConfigurationService, MockMessagingService, MockMetricService

logger = logging.getLogger(__name__)


class TestableGGCommons(GGCommons):
    """
    Testable GGCommons that allows service injection before initialization.
    
    This enables true unit testing by injecting mocks before any real services are created.
    Perfect for testing component behavior in isolation.
    """
    
    def __init__(self, component_name: str, args: List[str], 
                 initial_config: Optional[Dict[str, Any]] = None,
                 auto_inject_mocks: bool = True):
        """
        Initialize testable GGCommons with optional mock injection.
        
        Args:
            component_name: The component name
            args: Command line arguments
            initial_config: Initial configuration for mock config service
            auto_inject_mocks: Whether to automatically inject mock services
            
        Raises:
            ValueError: If component_name is None or empty
        """
        if not component_name:
            raise ValueError("Component name cannot be None or empty")
            
        self._component_name = component_name
        self._args = args
        self._initial_config = initial_config
        
        try:
            # Initialize service registry first
            self._service_registry = ServiceRegistry()
            
            # Inject mock services if requested
            if auto_inject_mocks:
                self._inject_mock_services()
                
            # Initialize configuration manager with mock or real implementation
            self._init_config_manager_for_testing()
            
            logger.info(f"TestableGGCommons initialized for component: {component_name}")
            
        except Exception as e:
            logger.error(f"Failed to initialize TestableGGCommons: {e}")
            raise
            
    def _inject_mock_services(self) -> None:
        """Inject mock services into the service registry."""
        # Create mock services
        mock_config = MockConfigurationService(self._initial_config)
        mock_messaging = MockMessagingService()
        mock_metrics = MockMetricService()
        
        # Register mock services
        self._service_registry.register(IConfigurationService, mock_config)
        self._service_registry.register(IMessagingService, mock_messaging)
        self._service_registry.register(IMetricService, mock_metrics)
        
        logger.debug("Mock services injected into TestableGGCommons")
        
    def _init_config_manager_for_testing(self) -> None:
        """Initialize configuration manager for testing."""
        # Get config service (mock or real)
        config_service = self._service_registry.get(IConfigurationService)
        
        if isinstance(config_service, MockConfigurationService):
            # Use the mock's underlying config manager interface
            self._config_manager = config_service
        else:
            # Initialize real config manager
            from ggcommons.config.manager.enhanced_config_manager import EnhancedConfigManager
            self._config_manager = EnhancedConfigManager(self._component_name, validate_config=False)
            
    def get_mock_messaging_service(self) -> Optional[MockMessagingService]:
        """
        Get the mock messaging service if available.
        
        Returns:
            Mock messaging service or None if not using mocks
        """
        messaging_service = self._service_registry.get(IMessagingService)
        if isinstance(messaging_service, MockMessagingService):
            return messaging_service
        return None
        
    def get_mock_configuration_service(self) -> Optional[MockConfigurationService]:
        """
        Get the mock configuration service if available.
        
        Returns:
            Mock configuration service or None if not using mocks
        """
        config_service = self._service_registry.get(IConfigurationService)
        if isinstance(config_service, MockConfigurationService):
            return config_service
        return None
        
    def get_mock_metric_service(self) -> Optional[MockMetricService]:
        """
        Get the mock metric service if available.
        
        Returns:
            Mock metric service or None if not using mocks
        """
        metric_service = self._service_registry.get(IMetricService)
        if isinstance(metric_service, MockMetricService):
            return metric_service
        return None
        
    def inject_custom_service(self, service_type, implementation) -> None:
        """
        Inject a custom service implementation.
        
        Args:
            service_type: The service interface type
            implementation: The service implementation
        """
        self._service_registry.register(service_type, implementation)
        logger.debug(f"Custom service injected: {service_type}")
        
    def reset_mocks(self) -> None:
        """Reset all mock services to clean state."""
        mock_messaging = self.get_mock_messaging_service()
        if mock_messaging:
            mock_messaging.clear_published_messages()
            mock_messaging.subscriptions.clear()
            mock_messaging.request_responses.clear()
            
        mock_metrics = self.get_mock_metric_service()
        if mock_metrics:
            mock_metrics.clear_emitted_metrics()
            mock_metrics.defined_metrics.clear()
            
        logger.debug("Mock services reset to clean state")
        
    def simulate_message_received(self, topic: str, message: Any) -> None:
        """
        Simulate receiving a message for testing.
        
        Args:
            topic: The topic the message was received on
            message: The message content
        """
        mock_messaging = self.get_mock_messaging_service()
        if mock_messaging:
            mock_messaging.inject_message(topic, message)
        else:
            logger.warning("Cannot simulate message - no mock messaging service available")
            
    def simulate_config_change(self, new_config: Dict[str, Any]) -> None:
        """
        Simulate a configuration change for testing.
        
        Args:
            new_config: The new configuration
        """
        mock_config = self.get_mock_configuration_service()
        if mock_config:
            mock_config.trigger_config_change(new_config)
        else:
            logger.warning("Cannot simulate config change - no mock configuration service available")
            
    def verify_message_published(self, topic: str, expected_count: int = 1) -> bool:
        """
        Verify that a message was published to a topic.
        
        Args:
            topic: The topic to check
            expected_count: Expected number of messages
            
        Returns:
            True if the expected number of messages were published
        """
        mock_messaging = self.get_mock_messaging_service()
        if mock_messaging:
            messages = mock_messaging.get_published_messages_for_topic(topic)
            return len(messages) == expected_count
        return False
        
    def verify_metric_emitted(self, metric_name: str, expected_count: int = 1) -> bool:
        """
        Verify that a metric was emitted.
        
        Args:
            metric_name: The metric name to check
            expected_count: Expected number of emissions
            
        Returns:
            True if the expected number of metrics were emitted
        """
        mock_metrics = self.get_mock_metric_service()
        if mock_metrics:
            metrics = mock_metrics.get_emitted_metrics_for_name(metric_name)
            return len(metrics) == expected_count
        return False
        
    def get_published_messages(self) -> List:
        """
        Get all published messages for verification.
        
        Returns:
            List of published messages
        """
        mock_messaging = self.get_mock_messaging_service()
        if mock_messaging:
            return mock_messaging.published_messages
        return []
        
    def get_emitted_metrics(self) -> List:
        """
        Get all emitted metrics for verification.
        
        Returns:
            List of emitted metrics
        """
        mock_metrics = self.get_mock_metric_service()
        if mock_metrics:
            return mock_metrics.emitted_metrics
        return []
        
    def set_request_response(self, topic: str, response: Any) -> None:
        """
        Set a response for request-response testing.
        
        Args:
            topic: The request topic
            response: The response to return
        """
        mock_messaging = self.get_mock_messaging_service()
        if mock_messaging:
            mock_messaging.set_request_response(topic, response)
        else:
            logger.warning("Cannot set request response - no mock messaging service available")


class TestContext:
    """
    Test context manager for setting up and tearing down test environments.
    
    Provides a convenient way to set up testable ggcommons instances with
    proper cleanup after tests complete.
    """
    
    def __init__(self, component_name: str, initial_config: Optional[Dict[str, Any]] = None):
        """
        Initialize test context.
        
        Args:
            component_name: The component name for testing
            initial_config: Initial configuration for testing
        """
        self.component_name = component_name
        self.initial_config = initial_config
        self.ggcommons: Optional[TestableGGCommons] = None
        
    def __enter__(self) -> TestableGGCommons:
        """Enter test context and create testable ggcommons."""
        self.ggcommons = TestableGGCommons(
            self.component_name, 
            [], 
            self.initial_config
        )
        return self.ggcommons
        
    def __exit__(self, exc_type, exc_val, exc_tb):
        """Exit test context and cleanup resources."""
        if self.ggcommons:
            try:
                # Reset mocks
                self.ggcommons.reset_mocks()
                
                # Could add additional cleanup here
                logger.debug(f"Test context cleaned up for {self.component_name}")
                
            except Exception as e:
                logger.warning(f"Error during test context cleanup: {e}")
                
        return False  # Don't suppress exceptions


# Convenience functions for common test scenarios
def create_test_ggcommons(component_name: str, 
                         config: Optional[Dict[str, Any]] = None) -> TestableGGCommons:
    """
    Create a testable ggcommons instance with default mock services.
    
    Args:
        component_name: The component name
        config: Optional initial configuration
        
    Returns:
        Configured TestableGGCommons instance
    """
    return TestableGGCommons(component_name, [], config)


def create_minimal_config() -> Dict[str, Any]:
    """
    Create a minimal configuration for testing.
    
    Returns:
        Minimal configuration dictionary
    """
    return {
        'component': {
            'global': {},
            'instances': [
                {'id': 'test-instance'}
            ]
        },
        'logging': {
            'level': 'DEBUG'
        },
        'heartbeat': {
            'intervalSecs': 1,
            'targets': [{'type': 'metric'}]
        }
    }