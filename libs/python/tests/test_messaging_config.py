"""
Unit tests for messaging configuration classes.
"""

import pytest
import tempfile
import json
import os

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
    pytest.skip("AWS SDK dependencies not available", allow_module_level=True)


# Fixtures
@pytest.fixture
def valid_config():
    """Valid messaging configuration for testing."""
    return {
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


@pytest.fixture
def temp_config_file(valid_config):
    """Create temporary config file."""
    with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
        json.dump(valid_config, f)
        temp_path = f.name
    
    yield temp_path
    
    # Cleanup
    if os.path.exists(temp_path):
        os.unlink(temp_path)


# CredentialsConfig tests
def test_credentials_config_init_default():
    """Test default initialization."""
    creds = CredentialsConfig()
    assert creds.username is None
    assert creds.password is None
    assert creds.cert_path is None
    assert creds.key_path is None
    assert creds.ca_path is None


def test_credentials_config_init_with_values():
    """Test initialization with values."""
    creds = CredentialsConfig(
        username="user",
        password="pass",
        cert_path="cert.pem",
        key_path="key.pem",
        ca_path="ca.pem"
    )
    assert creds.username == "user"
    assert creds.password == "pass"
    assert creds.cert_path == "cert.pem"
    assert creds.key_path == "key.pem"
    assert creds.ca_path == "ca.pem"


# LocalMqttConfig tests
def test_local_mqtt_config_init_required_fields():
    """Test initialization with required fields."""
    config = LocalMqttConfig(
        type="mqtt",
        host="localhost",
        port=1883,
        client_id="test-client"
    )
    assert config.type == "mqtt"
    assert config.host == "localhost"
    assert config.port == 1883
    assert config.client_id == "test-client"
    assert config.credentials is None


def test_local_mqtt_config_init_with_credentials():
    """Test initialization with credentials."""
    creds = CredentialsConfig(username="user", password="pass")
    config = LocalMqttConfig(
        type="mqtt",
        host="localhost",
        port=1883,
        client_id="test-client",
        credentials=creds
    )
    assert config.credentials == creds


# IoTCoreConfig tests
def test_iot_core_config_init():
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
    assert config.endpoint == "test.iot.amazonaws.com"
    assert config.port == 8883
    assert config.client_id == "iot-client"
    assert config.credentials == creds


# MessagingConfiguration tests
def test_messaging_configuration_init():
    """Test initialization."""
    config = MessagingConfiguration()
    assert config.messaging is None


def test_messaging_configuration_load_from_file_valid(temp_config_file):
    """Test loading valid configuration from file."""
    config = MessagingConfiguration.load_from_file(temp_config_file)
    
    # Verify structure
    assert config.messaging is not None
    assert config.messaging.local is not None
    assert config.messaging.iot_core is not None
    
    # Verify local config
    local = config.messaging.local
    assert local.type == "mqtt"
    assert local.host == "localhost"
    assert local.port == 1883
    assert local.client_id == "local-client"
    assert local.credentials is not None
    assert local.credentials.username == "user"
    assert local.credentials.password == "pass"
    
    # Verify IoT Core config
    iot_core = config.messaging.iot_core
    assert iot_core.endpoint == "test.iot.amazonaws.com"
    assert iot_core.port == 8883
    assert iot_core.client_id == "iot-client"
    assert iot_core.credentials is not None
    assert iot_core.credentials.cert_path == "cert.pem"
    assert iot_core.credentials.key_path == "key.pem"
    assert iot_core.credentials.ca_path == "ca.pem"


def test_messaging_configuration_load_from_file_iot_core_only():
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
        assert config.messaging is not None
        assert config.messaging.local is None
        assert config.messaging.iot_core is not None
    finally:
        os.unlink(temp_path)


def test_messaging_configuration_load_from_file_missing_iot_core():
    """Loading a local-only config (no IoT Core) is valid: IoT Core is optional."""
    local_only_config = {
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
        json.dump(local_only_config, f)
        temp_path = f.name

    try:
        config = MessagingConfiguration.load_from_file(temp_path)
        assert config.messaging.local is not None
        assert config.messaging.iot_core is None
    finally:
        os.unlink(temp_path)


def test_messaging_configuration_load_from_file_not_found():
    """Test loading from non-existent file."""
    with pytest.raises(Exception):
        MessagingConfiguration.load_from_file("non_existent_file.json")


def test_messaging_configuration_validate_valid_config(temp_config_file):
    """Test validation of valid configuration."""
    config = MessagingConfiguration.load_from_file(temp_config_file)
    assert config.validate() is True


def test_messaging_configuration_validate_no_messaging():
    """Test validation with no messaging configuration."""
    config = MessagingConfiguration()
    assert config.validate() is False


def test_messaging_configuration_validate_no_iot_core():
    """Test validation with no IoT Core configuration."""
    config = MessagingConfiguration()
    config.messaging = MessagingConfigData()
    assert config.validate() is False


def test_messaging_configuration_validate_iot_core_missing_credentials():
    """Test validation with IoT Core missing credentials."""
    config = MessagingConfiguration()
    config.messaging = MessagingConfigData()
    config.messaging.iot_core = IoTCoreConfig(
        endpoint="test.iot.amazonaws.com",
        port=8883,
        client_id="test",
        credentials=CredentialsConfig()  # Empty credentials
    )
    assert config.validate() is False