"""
Integration tests for GGCommons system.
"""

import unittest
import tempfile
import json
import os
from unittest.mock import Mock, patch

try:
    from ggcommons.ggcommons_builder import GGCommonsBuilder
    from ggcommons.interfaces import IMessagingService, IConfigurationService, IMetricService
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestGGCommonsIntegration(unittest.TestCase):
    """Integration tests for GGCommons system."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.temp_files = []
    
    def tearDown(self):
        """Clean up after tests."""
        for temp_file in self.temp_files:
            if os.path.exists(temp_file):
                os.unlink(temp_file)
    
    def create_temp_config_file(self, config_data):
        """Create temporary configuration file."""
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
            json.dump(config_data, f)
            self.temp_files.append(f.name)
            return f.name
    
    @patch('ggcommons.messaging.messaging_client.GreengrassIpcProvider')
    def test_basic_initialization(self, mock_ipc_provider):
        """Test basic GGCommons initialization."""
        mock_provider = Mock()
        mock_ipc_provider.return_value = mock_provider
        
        # Create basic configuration
        config = {
            "logging": {"level": "INFO"},
            "component": {
                "global": {"setting": "value"},
                "instances": [{"id": "main"}]
            }
        }
        config_file = self.create_temp_config_file(config)
        
        # Initialize GGCommons
        args = ["-c", "FILE", config_file, "-t", "test-thing"]
        ggcommons = GGCommonsBuilder.create("com.test.Component") \
            .with_args(args) \
            .build()
        
        # Verify services are available
        config_service = ggcommons.get_service(IConfigurationService)
        messaging_service = ggcommons.get_service(IMessagingService)
        metric_service = ggcommons.get_service(IMetricService)
        
        self.assertIsNotNone(config_service)
        self.assertIsNotNone(messaging_service)
        self.assertIsNotNone(metric_service)
    
    def test_standalone_mode_configuration_loading(self):
        """Test STANDALONE mode configuration loading."""
        # Create messaging configuration
        messaging_config = {
            "messaging": {
                "iotCore": {
                    "endpoint": "test.iot.amazonaws.com",
                    "port": 8883,
                    "clientId": "test-client",
                    "credentials": {
                        "certPath": "cert.pem",
                        "keyPath": "key.pem",
                        "caPath": "ca.pem"
                    }
                }
            }
        }
        messaging_config_file = self.create_temp_config_file(messaging_config)
        
        # Create component configuration
        component_config = {
            "logging": {"level": "INFO"},
            "component": {
                "global": {"setting": "value"},
                "instances": [{"id": "main"}]
            }
        }
        component_config_file = self.create_temp_config_file(component_config)
        
        # Test configuration loading (without actual initialization to avoid dependencies)
        from ggcommons.messaging.messaging_config import MessagingConfiguration
        
        config = MessagingConfiguration.load_from_file(messaging_config_file)
        self.assertTrue(config.validate())
        self.assertIsNotNone(config.messaging.iot_core)
        self.assertEqual(config.messaging.iot_core.endpoint, "test.iot.amazonaws.com")
    
    @patch('ggcommons.messaging.messaging_client.GreengrassIpcProvider')
    def test_service_injection(self, mock_ipc_provider):
        """Test service injection and retrieval."""
        mock_provider = Mock()
        mock_ipc_provider.return_value = mock_provider
        
        config = {
            "logging": {"level": "INFO"},
            "component": {
                "global": {"setting": "value"},
                "instances": [{"id": "main"}]
            }
        }
        config_file = self.create_temp_config_file(config)
        
        args = ["-c", "FILE", config_file, "-t", "test-thing"]
        ggcommons = GGCommonsBuilder.create("com.test.Component") \
            .with_args(args) \
            .build()
        
        # Test custom service registration
        custom_service = Mock()
        ggcommons.register_service(IMessagingService, custom_service)
        
        # Verify custom service is returned
        retrieved_service = ggcommons.get_service(IMessagingService)
        self.assertEqual(retrieved_service, custom_service)
    
    @patch('ggcommons.messaging.messaging_client.GreengrassIpcProvider')
    def test_configuration_access(self, mock_ipc_provider):
        """Test configuration access through services."""
        mock_provider = Mock()
        mock_ipc_provider.return_value = mock_provider
        
        config = {
            "logging": {"level": "DEBUG"},
            "component": {
                "global": {"globalSetting": "globalValue"},
                "instances": [
                    {
                        "id": "instance1",
                        "instanceSetting": "instanceValue"
                    }
                ]
            }
        }
        config_file = self.create_temp_config_file(config)
        
        args = ["-c", "FILE", config_file, "-t", "test-thing"]
        ggcommons = GGCommonsBuilder.create("com.test.Component") \
            .with_args(args) \
            .build()
        
        # Test configuration access
        config_service = ggcommons.get_service(IConfigurationService)
        
        # Test global configuration
        global_config = config_service.get_global_config()
        self.assertEqual(global_config["globalSetting"], "globalValue")
        
        # Test instance configuration
        instance_ids = config_service.get_instance_ids()
        self.assertIn("instance1", instance_ids)
        
        instance_config = config_service.get_instance_config("instance1")
        self.assertEqual(instance_config["instanceSetting"], "instanceValue")
    
    def test_builder_pattern_fluent_interface(self):
        """Test builder pattern fluent interface."""
        from ggcommons.messaging.message_builder import MessageBuilder
        from ggcommons.metrics.metric_builder import MetricBuilder
        
        # Test MessageBuilder fluent interface
        message = MessageBuilder.create("TestMessage", "1.0") \
            .with_payload({"data": "test"}) \
            .with_correlation_id("test-123") \
            .with_reply_to("reply/topic") \
            .build()
        
        self.assertIsNotNone(message)
        
        # Test MetricBuilder fluent interface
        metric = MetricBuilder.create("test_metric") \
            .with_namespace("TestApp/Metrics") \
            .add_measure("count", "Count", 1.0) \
            .add_dimension("instance", "main") \
            .build()
        
        self.assertIsNotNone(metric)
        self.assertEqual(metric.name, "test_metric")
        self.assertEqual(metric.namespace, "TestApp/Metrics")
    
    def test_error_handling(self):
        """Test error handling in various scenarios."""
        # Test invalid component name
        with self.assertRaises(ValueError):
            GGCommonsBuilder.create("")
        
        # Test invalid configuration file
        args = ["-c", "FILE", "nonexistent.json", "-t", "test-thing"]
        with self.assertRaises(Exception):
            GGCommonsBuilder.create("com.test.Component") \
                .with_args(args) \
                .build()
    
    def test_service_registry_isolation(self):
        """Test that different GGCommons instances have isolated service registries."""
        config = {
            "logging": {"level": "INFO"},
            "component": {
                "global": {"setting": "value"},
                "instances": [{"id": "main"}]
            }
        }
        config_file1 = self.create_temp_config_file(config)
        config_file2 = self.create_temp_config_file(config)
        
        with patch('ggcommons.messaging.messaging_client.GreengrassIpcProvider') as mock_ipc:
            mock_ipc.return_value = Mock()
            
            # Create two GGCommons instances
            args1 = ["-c", "FILE", config_file1, "-t", "thing1"]
            ggcommons1 = GGCommonsBuilder.create("com.test.Component1") \
                .with_args(args1) \
                .build()
            
            args2 = ["-c", "FILE", config_file2, "-t", "thing2"]
            ggcommons2 = GGCommonsBuilder.create("com.test.Component2") \
                .with_args(args2) \
                .build()
            
            # Register different services in each instance
            custom_service1 = Mock()
            custom_service2 = Mock()
            
            ggcommons1.register_service(IMessagingService, custom_service1)
            ggcommons2.register_service(IMessagingService, custom_service2)
            
            # Verify isolation
            service1 = ggcommons1.get_service(IMessagingService)
            service2 = ggcommons2.get_service(IMessagingService)
            
            self.assertEqual(service1, custom_service1)
            self.assertEqual(service2, custom_service2)
            self.assertNotEqual(service1, service2)


if __name__ == '__main__':
    unittest.main()