"""
Unit tests for messaging configuration classes.
"""

import unittest
import tempfile
import json
import os
from unittest.mock import patch, mock_open

# Mock the AWS SDK import to avoid dependency issues in tests
try:
    from ggcommons.messaging.messaging_config import (
        MessagingConfiguration,
        MessagingConfigData,
        LocalMqttConfig,
        IoTCoreConfig,
        CredentialsConfig
    )
except ImportError:
    # Skip tests if dependencies not available
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestCredentialsConfig(unittest.TestCase):
    """Test CredentialsConfig class."""
    
    def test_init_default(self):
        """Test default initialization."""
        creds = CredentialsConfig()
        self.assertIsNone(creds.username)
        self.assertIsNone(creds.password)
        self.assertIsNone(creds.cert_path)
        self.assertIsNone(creds.key_path)
        self.assertIsNone(creds.ca_path)
    
    def test_init_with_values(self):
        """Test initialization with values."""
        creds = CredentialsConfig(
            username="user",
            password="pass",
            cert_path="cert.pem",
            key_path="key.pem",
            ca_path="ca.pem"
        )
        self.assertEqual(creds.username, "user")
        self.assertEqual(creds.password, "pass")
        self.assertEqual(creds.cert_path, "cert.pem")
        self.assertEqual(creds.key_path, "key.pem")
        self.assertEqual(creds.ca_path, "ca.pem")


class TestLocalMqttConfig(unittest.TestCase):
    """Test LocalMqttConfig class."""
    
    def test_init_required_fields(self):
        """Test initialization with required fields."""
        config = LocalMqttConfig(
            type="mqtt",
            host="localhost",
            port=1883,
            client_id="test-client"
        )
        self.assertEqual(config.type, "mqtt")
        self.assertEqual(config.host, "localhost")
        self.assertEqual(config.port, 1883)
        self.assertEqual(config.client_id, "test-client")
        self.assertIsNone(config.credentials)
    
    def test_init_with_credentials(self):
        """Test initialization with credentials."""
        creds = CredentialsConfig(username="user", password="pass")
        config = LocalMqttConfig(
            type="mqtt",
            host="localhost",
            port=1883,
            client_id="test-client",
            credentials=creds
        )
        self.assertEqual(config.credentials, creds)


class TestIoTCoreConfig(unittest.TestCase):
    """Test IoTCoreConfig class."""
    
    def test_init(self):
        """Test initialization."""
        creds = CredentialsConfig(
            cert_path="cert.pem",
            key_path="key.pem",
            ca_path="ca.pem"
        )
        config = IoTCoreConfig(
            endpoint="test.iot.amazonaws.com",
            port=8883,
            client_id="iot-client",
            credentials=creds
        )
        self.assertEqual(config.endpoint, "test.iot.amazonaws.com")
        self.assertEqual(config.port, 8883)
        self.assertEqual(config.client_id, "iot-client")
        self.assertEqual(config.credentials, creds)


class TestMessagingConfiguration(unittest.TestCase):
    """Test MessagingConfiguration class."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.valid_config = {
            "messaging": {
                "local": {
                    "type": "mqtt",
                    "host": "localhost",
                    "port": 1883,
                    "clientId": "local-client",
                    "credentials": {
                        "username": "user",
                        "password": "pass"
                    }
                },
                "iotCore": {
                    "endpoint": "test.iot.amazonaws.com",
                    "port": 8883,
                    "clientId": "iot-client",
                    "credentials": {
                        "certPath": "cert.pem",
                        "keyPath": "key.pem",
                        "caPath": "ca.pem"
                    }
                }
            }
        }
    
    def test_init(self):
        """Test initialization."""
        config = MessagingConfiguration()
        self.assertIsNone(config.messaging)
    
    def test_load_from_file_valid(self):
        """Test loading valid configuration from file."""
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
            json.dump(self.valid_config, f)
            temp_path = f.name
        
        try:
            config = MessagingConfiguration.load_from_file(temp_path)
            
            # Verify structure
            self.assertIsNotNone(config.messaging)
            self.assertIsNotNone(config.messaging.local)
            self.assertIsNotNone(config.messaging.iot_core)
            
            # Verify local config
            local = config.messaging.local
            self.assertEqual(local.type, "mqtt")
            self.assertEqual(local.host, "localhost")
            self.assertEqual(local.port, 1883)
            self.assertEqual(local.client_id, "local-client")
            self.assertIsNotNone(local.credentials)
            self.assertEqual(local.credentials.username, "user")
            self.assertEqual(local.credentials.password, "pass")
            
            # Verify IoT Core config
            iot_core = config.messaging.iot_core
            self.assertEqual(iot_core.endpoint, "test.iot.amazonaws.com")
            self.assertEqual(iot_core.port, 8883)
            self.assertEqual(iot_core.client_id, "iot-client")
            self.assertIsNotNone(iot_core.credentials)
            self.assertEqual(iot_core.credentials.cert_path, "cert.pem")
            self.assertEqual(iot_core.credentials.key_path, "key.pem")
            self.assertEqual(iot_core.credentials.ca_path, "ca.pem")
            
        finally:
            os.unlink(temp_path)
    
    def test_load_from_file_iot_core_only(self):
        """Test loading configuration with IoT Core only."""
        iot_only_config = {
            "messaging": {
                "iotCore": {
                    "endpoint": "test.iot.amazonaws.com",
                    "port": 8883,
                    "clientId": "iot-client",
                    "credentials": {
                        "certPath": "cert.pem",
                        "keyPath": "key.pem",
                        "caPath": "ca.pem"
                    }
                }
            }
        }
        
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
            json.dump(iot_only_config, f)
            temp_path = f.name
        
        try:
            config = MessagingConfiguration.load_from_file(temp_path)
            self.assertIsNotNone(config.messaging)
            self.assertIsNone(config.messaging.local)
            self.assertIsNotNone(config.messaging.iot_core)
        finally:
            os.unlink(temp_path)
    
    def test_load_from_file_missing_iot_core(self):
        """Test loading configuration without IoT Core (should fail)."""
        invalid_config = {
            "messaging": {
                "local": {
                    "type": "mqtt",
                    "host": "localhost",
                    "port": 1883,
                    "clientId": "local-client"
                }
            }
        }
        
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
            json.dump(invalid_config, f)
            temp_path = f.name
        
        try:
            with self.assertRaises(ValueError) as context:
                MessagingConfiguration.load_from_file(temp_path)
            self.assertIn("IoT Core configuration is required", str(context.exception))
        finally:
            os.unlink(temp_path)
    
    def test_load_from_file_not_found(self):
        """Test loading from non-existent file."""
        with self.assertRaises(Exception):
            MessagingConfiguration.load_from_file("non_existent_file.json")
    
    def test_validate_valid_config(self):
        """Test validation of valid configuration."""
        with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
            json.dump(self.valid_config, f)
            temp_path = f.name
        
        try:
            config = MessagingConfiguration.load_from_file(temp_path)
            self.assertTrue(config.validate())
        finally:
            os.unlink(temp_path)
    
    def test_validate_no_messaging(self):
        """Test validation with no messaging configuration."""
        config = MessagingConfiguration()
        self.assertFalse(config.validate())
    
    def test_validate_no_iot_core(self):
        """Test validation with no IoT Core configuration."""
        config = MessagingConfiguration()
        config.messaging = MessagingConfigData()
        self.assertFalse(config.validate())
    
    def test_validate_iot_core_missing_credentials(self):
        """Test validation with IoT Core missing credentials."""
        config = MessagingConfiguration()
        config.messaging = MessagingConfigData()
        config.messaging.iot_core = IoTCoreConfig(
            endpoint="test.iot.amazonaws.com",
            port=8883,
            client_id="test",
            credentials=CredentialsConfig()  # Empty credentials
        )
        self.assertFalse(config.validate())


if __name__ == '__main__':
    unittest.main()