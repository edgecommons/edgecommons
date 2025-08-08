"""
Unit tests for messaging client.
"""

import unittest
from unittest.mock import Mock, patch, MagicMock
from argparse import Namespace

try:
    from ggcommons.messaging.messaging_client import MessagingClient
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestMessagingClient(unittest.TestCase):
    """Test MessagingClient class."""
    
    def setUp(self):
        """Set up test fixtures."""
        # Reset static provider
        MessagingClient._messaging_provider = None
    
    def tearDown(self):
        """Clean up after tests."""
        MessagingClient._messaging_provider = None
    
    @patch('ggcommons.messaging.messaging_client.GreengrassIpcProvider')
    def test_init_greengrass_mode(self, mock_greengrass_provider):
        """Test initialization in Greengrass mode."""
        mock_provider = Mock()
        mock_greengrass_provider.return_value = mock_provider
        
        args = Namespace(mode=['GREENGRASS'], thing='test-thing')
        
        result = MessagingClient.init(args)
        
        self.assertEqual(result, mock_provider)
        self.assertEqual(MessagingClient._messaging_provider, mock_provider)
        mock_greengrass_provider.assert_called_once_with(False)
    
    @patch('ggcommons.messaging.messaging_client.MessagingConfiguration')
    @patch('ggcommons.messaging.messaging_client.StandaloneProvider')
    def test_init_standalone_mode(self, mock_standalone_provider, mock_config_class):
        """Test initialization in STANDALONE mode."""
        mock_provider = Mock()
        mock_standalone_provider.return_value = mock_provider
        mock_config = Mock()
        mock_config_class.load_from_file.return_value = mock_config
        mock_config.validate.return_value = True
        
        args = Namespace(mode=['STANDALONE'], thing='test-thing')
        config_path = 'test-config.json'
        
        result = MessagingClient.init(args, config_path)
        
        self.assertEqual(result, mock_provider)
        self.assertEqual(MessagingClient._messaging_provider, mock_provider)
        mock_config_class.load_from_file.assert_called_once_with(config_path)
        mock_config.validate.assert_called_once()
        mock_standalone_provider.assert_called_once_with(mock_config, 'test-thing')
    
    def test_init_standalone_no_config_path(self):
        """Test STANDALONE mode without config path raises error."""
        args = Namespace(mode=['STANDALONE'], thing='test-thing')
        
        with self.assertRaises(RuntimeError) as context:
            MessagingClient.init(args)
        
        self.assertIn("STANDALONE mode requires standalone config file path", str(context.exception))
    
    @patch('ggcommons.messaging.messaging_client.MessagingConfiguration')
    def test_init_standalone_invalid_config(self, mock_config_class):
        """Test STANDALONE mode with invalid config raises error."""
        mock_config = Mock()
        mock_config_class.load_from_file.return_value = mock_config
        mock_config.validate.return_value = False
        
        args = Namespace(mode=['STANDALONE'], thing='test-thing')
        config_path = 'test-config.json'
        
        with self.assertRaises(RuntimeError) as context:
            MessagingClient.init(args, config_path)
        
        self.assertIn("Invalid messaging configuration", str(context.exception))
    
    def test_publish_not_initialized(self):
        """Test publish when not initialized raises error."""
        with self.assertRaises(RuntimeError):
            MessagingClient.publish("test/topic", Mock())
    
    def test_publish_initialized(self):
        """Test publish when initialized."""
        mock_provider = Mock()
        MessagingClient._messaging_provider = mock_provider
        message = Mock()
        
        MessagingClient.publish("test/topic", message)
        
        mock_provider.publish.assert_called_once_with("test/topic", message)
    
    def test_shutdown(self):
        """Test shutdown."""
        mock_provider = Mock()
        MessagingClient._messaging_provider = mock_provider
        
        MessagingClient.shutdown()
        
        mock_provider.disconnect.assert_called_once()
        self.assertIsNone(MessagingClient._messaging_provider)
    
    def test_get_messaging_provider(self):
        """Test get messaging provider."""
        mock_provider = Mock()
        MessagingClient._messaging_provider = mock_provider
        
        result = MessagingClient.get_messaging_provider()
        
        self.assertEqual(result, mock_provider)
    
    def test_all_methods_delegate_to_provider(self):
        """Test that all methods delegate to the provider."""
        mock_provider = Mock()
        MessagingClient._messaging_provider = mock_provider
        
        # Test all delegation methods
        message = Mock()
        qos = Mock()
        callback = Mock()
        iou = Mock()
        
        MessagingClient.publish("topic", message)
        mock_provider.publish.assert_called_with("topic", message)
        
        MessagingClient.publish_raw("topic", {"data": "test"})
        mock_provider.publish_raw.assert_called_with("topic", {"data": "test"})
        
        MessagingClient.publish_to_iot_core("topic", message, qos)
        mock_provider.publish_to_iot_core.assert_called_with("topic", message, qos)
        
        MessagingClient.subscribe("topic", callback, 5)
        mock_provider.subscribe.assert_called_with("topic", callback, 5)
        
        MessagingClient.request("topic", message)
        mock_provider.request.assert_called_with("topic", message)
        
        MessagingClient.reply(message, message)
        mock_provider.reply.assert_called_with(message, message)


if __name__ == '__main__':
    unittest.main()