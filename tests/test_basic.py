"""
Basic tests that don't require external dependencies.
"""

import unittest
import sys
import os

# Add parent directory to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))


class TestBasicFunctionality(unittest.TestCase):
    """Test basic functionality without external dependencies."""
    
    def test_messaging_config_classes_exist(self):
        """Test that messaging config classes can be imported."""
        try:
            from ggcommons.messaging.messaging_config import (
                MessagingConfiguration,
                CredentialsConfig,
                LocalMqttConfig,
                IoTCoreConfig
            )
            self.assertTrue(True, "Classes imported successfully")
        except ImportError as e:
            self.skipTest(f"Skipping due to missing dependencies: {e}")
    
    def test_java_compatible_config_structure(self):
        """Test Java-compatible configuration structure."""
        try:
            from ggcommons.messaging.messaging_config import (
                MessagingConfiguration,
                CredentialsConfig,
                LocalMqttConfig,
                IoTCoreConfig
            )
            
            # Test CredentialsConfig
            creds = CredentialsConfig(
                username="test",
                password="pass",
                cert_path="cert.pem",
                key_path="key.pem",
                ca_path="ca.pem"
            )
            self.assertEqual(creds.username, "test")
            self.assertEqual(creds.cert_path, "cert.pem")
            
            # Test LocalMqttConfig
            local_config = LocalMqttConfig(
                type="mqtt",
                host="localhost",
                port=1883,
                client_id="test-client",
                credentials=creds
            )
            self.assertEqual(local_config.type, "mqtt")
            self.assertEqual(local_config.host, "localhost")
            self.assertEqual(local_config.port, 1883)
            
            # Test IoTCoreConfig
            iot_creds = CredentialsConfig(
                cert_path="cert.pem",
                key_path="key.pem",
                ca_path="ca.pem"
            )
            iot_config = IoTCoreConfig(
                endpoint="test.iot.amazonaws.com",
                port=8883,
                client_id="iot-client",
                credentials=iot_creds
            )
            self.assertEqual(iot_config.endpoint, "test.iot.amazonaws.com")
            self.assertEqual(iot_config.port, 8883)
            
        except ImportError as e:
            self.skipTest(f"Skipping due to missing dependencies: {e}")
    
    def test_configuration_file_loading(self):
        """Test configuration file loading mechanism."""
        import tempfile
        import json
        
        try:
            from ggcommons.messaging.messaging_config import MessagingConfiguration
            
            # Create test config
            config_data = {
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
            
            # Write to temp file
            with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
                json.dump(config_data, f)
                temp_path = f.name
            
            try:
                # Load configuration
                config = MessagingConfiguration.load_from_file(temp_path)
                
                # Verify structure
                self.assertIsNotNone(config.messaging)
                self.assertIsNotNone(config.messaging.iot_core)
                self.assertEqual(config.messaging.iot_core.endpoint, "test.iot.amazonaws.com")
                
                # Test validation
                self.assertTrue(config.validate())
                
            finally:
                os.unlink(temp_path)
                
        except ImportError as e:
            self.skipTest(f"Skipping due to missing dependencies: {e}")


if __name__ == '__main__':
    unittest.main()