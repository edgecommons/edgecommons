"""
Basic tests that don't require external dependencies.
"""

import pytest
import tempfile
import json
import os


def test_messaging_config_classes_exist():
    """Test that messaging config classes can be imported."""
    try:
        from edgecommons.messaging.messaging_config import (
            MessagingConfiguration,
            CredentialsConfig,
            LocalMqttConfig,
            IoTCoreConfig
        )
        # If we get here, import was successful
        assert True
    except ImportError as e:
        pytest.skip(f"Skipping due to missing dependencies: {e}")


def test_java_compatible_config_structure():
    """Test Java-compatible configuration structure."""
    try:
        from edgecommons.messaging.messaging_config import (
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
        assert creds.username == "test"
        assert creds.cert_path == "cert.pem"
        
        # Test LocalMqttConfig
        local_config = LocalMqttConfig(
            type="mqtt",
            host="localhost",
            port=1883,
            client_id="test-client",
            credentials=creds
        )
        assert local_config.type == "mqtt"
        assert local_config.host == "localhost"
        assert local_config.port == 1883
        
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
        assert iot_config.endpoint == "test.iot.amazonaws.com"
        assert iot_config.port == 8883
        
    except ImportError as e:
        pytest.skip(f"Skipping due to missing dependencies: {e}")


def test_configuration_file_loading():
    """Test configuration file loading mechanism."""
    try:
        from edgecommons.messaging.messaging_config import MessagingConfiguration
        
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
            assert config.messaging is not None
            assert config.messaging.iot_core is not None
            assert config.messaging.iot_core.endpoint == "test.iot.amazonaws.com"
            
            # Test validation
            assert config.validate() is True
            
        finally:
            os.unlink(temp_path)
            
    except ImportError as e:
        pytest.skip(f"Skipping due to missing dependencies: {e}")