"""
Unit tests for dependency injection system.
"""

import unittest
from unittest.mock import Mock

try:
    from ggcommons.di.service_registry import ServiceRegistry
    from ggcommons.di.service_factory import ServiceFactory
    from ggcommons.interfaces import IConfigurationService, IMessagingService, IMetricService
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestServiceRegistry(unittest.TestCase):
    """Test ServiceRegistry class."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.registry = ServiceRegistry()
    
    def test_register_and_get(self):
        """Test registering and retrieving services."""
        mock_service = Mock()
        
        self.registry.register(IMessagingService, mock_service)
        result = self.registry.get(IMessagingService)
        
        self.assertEqual(result, mock_service)
    
    def test_get_unregistered_service(self):
        """Test getting unregistered service returns None."""
        result = self.registry.get(IMessagingService)
        self.assertIsNone(result)
    
    def test_register_none_service_type(self):
        """Test registering with None service type raises error."""
        with self.assertRaises(ValueError):
            self.registry.register(None, Mock())
    
    def test_register_none_implementation(self):
        """Test registering with None implementation raises error."""
        with self.assertRaises(ValueError):
            self.registry.register(IMessagingService, None)
    
    def test_get_none_service_type(self):
        """Test getting with None service type raises error."""
        with self.assertRaises(ValueError):
            self.registry.get(None)
    
    def test_override_service(self):
        """Test overriding existing service."""
        mock_service1 = Mock()
        mock_service2 = Mock()
        
        self.registry.register(IMessagingService, mock_service1)
        self.registry.register(IMessagingService, mock_service2)
        
        result = self.registry.get(IMessagingService)
        self.assertEqual(result, mock_service2)
    
    def test_multiple_services(self):
        """Test registering multiple different services."""
        mock_messaging = Mock()
        mock_config = Mock()
        mock_metric = Mock()
        
        self.registry.register(IMessagingService, mock_messaging)
        self.registry.register(IConfigurationService, mock_config)
        self.registry.register(IMetricService, mock_metric)
        
        self.assertEqual(self.registry.get(IMessagingService), mock_messaging)
        self.assertEqual(self.registry.get(IConfigurationService), mock_config)
        self.assertEqual(self.registry.get(IMetricService), mock_metric)


class TestServiceFactory(unittest.TestCase):
    """Test ServiceFactory class."""
    
    def test_register_default_services(self):
        """Test registering default services."""
        registry = ServiceRegistry()
        mock_config_manager = Mock()
        
        ServiceFactory.register_default_services(registry, mock_config_manager)
        
        # Verify services are registered
        config_service = registry.get(IConfigurationService)
        messaging_service = registry.get(IMessagingService)
        metric_service = registry.get(IMetricService)
        
        self.assertIsNotNone(config_service)
        self.assertIsNotNone(messaging_service)
        self.assertIsNotNone(metric_service)
    
    def test_register_default_services_none_registry(self):
        """Test registering with None registry raises error."""
        with self.assertRaises(ValueError):
            ServiceFactory.register_default_services(None, Mock())
    
    def test_register_default_services_none_config_manager(self):
        """Test registering with None config manager raises error."""
        with self.assertRaises(ValueError):
            ServiceFactory.register_default_services(ServiceRegistry(), None)


if __name__ == '__main__':
    unittest.main()