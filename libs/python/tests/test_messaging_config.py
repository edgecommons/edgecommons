"""
Unit tests for messaging configuration classes.
"""

import pytest
import tempfile
import json
import os
from unittest.mock import MagicMock

# Mock the AWS SDK import to avoid dependency issues in tests
try:
    from ggcommons.messaging.messaging_config import (
        MessagingConfiguration,
        MessagingConfigData,
        LocalMqttConfig,
        IoTCoreConfig,
        CredentialsConfig
    )
    from ggcommons.messaging.providers.standalone_provider import StandaloneProvider
except ImportError:
    pytest.skip("AWS SDK dependencies not available", allow_module_level=True)


def _write_config(cfg):
    """Write a config dict to a temp JSON file and return its path (caller unlinks)."""
    with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
        json.dump(cfg, f)
        return f.name


def _provider_capturing_connects(config, monkeypatch, thing_name="thing-1"):
    """Build a StandaloneProvider with the MQTT client creation + connect stubbed out (no broker),
    capturing which channels were initialized and the host each connected to. Returns
    (provider, connected) where connected is a list of (channel_name, host) tuples."""
    connected = []
    monkeypatch.setattr(StandaloneProvider, "_create_mqtt_client", lambda self, bc, ch: MagicMock())

    def fake_connect(self, ch, bc):
        host = getattr(bc, "host", None) or getattr(bc, "endpoint", None)
        connected.append((ch.name, host))

    monkeypatch.setattr(StandaloneProvider, "_connect_client", fake_connect)
    provider = StandaloneProvider(config, thing_name)
    return provider, connected


def _local_section(host="localhost"):
    return {"type": "mqtt", "host": host, "port": 1883, "clientId": "local-client"}


def _iot_core_section(endpoint="test.iot.amazonaws.com"):
    return {
        "endpoint": endpoint,
        "port": 8883,
        "clientId": "iot-client",
        "credentials": {"certPath": "cert.pem", "keyPath": "key.pem", "caPath": "ca.pem"},
    }


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


# ---------- FR-MSG-2: a Kubernetes Service DNS name is an opaque host (accepted/used) ----------

def test_service_dns_host_is_accepted_for_local_broker():
    """The local broker host is an opaque string: a k8s Service DNS name loads with no special
    handling (FR-MSG-2)."""
    cfg = {"messaging": {"local": _local_section(host="emqx.mqtt.svc.cluster.local")}}
    path = _write_config(cfg)
    try:
        config = MessagingConfiguration.load_from_file(path)
        assert config.validate() is True
        assert config.messaging.local.host == "emqx.mqtt.svc.cluster.local"
    finally:
        os.unlink(path)


def test_service_dns_host_is_used_to_connect(monkeypatch):
    """The loaded Service DNS host is the one the provider connects to (FR-MSG-2) — no rewriting,
    no insecure downgrade."""
    cfg = {"messaging": {"local": _local_section(host="emqx.mqtt.svc.cluster.local")}}
    path = _write_config(cfg)
    try:
        config = MessagingConfiguration.load_from_file(path)
        provider, connected = _provider_capturing_connects(config, monkeypatch)
        try:
            assert ("local", "emqx.mqtt.svc.cluster.local") in connected
        finally:
            provider.disconnect()
    finally:
        os.unlink(path)


def test_service_dns_endpoint_is_accepted_for_iot_core():
    cfg = {"messaging": {"iotCore": _iot_core_section(endpoint="iot.endpoints.svc.cluster.local")}}
    path = _write_config(cfg)
    try:
        config = MessagingConfiguration.load_from_file(path)
        assert config.validate() is True
        assert config.messaging.iot_core.endpoint == "iot.endpoints.svc.cluster.local"
    finally:
        os.unlink(path)


# ---------- FR-MSG-3: single- (local only) vs dual- (local + IoT Core) MQTT topology ----------

def test_single_broker_topology_local_only(monkeypatch):
    """Air-gapped single-broker: only `messaging.local` -> the provider connects the local channel
    and never creates an IoT Core client (FR-MSG-3)."""
    cfg = {"messaging": {"local": _local_section(host="emqx.mqtt.svc.cluster.local")}}
    path = _write_config(cfg)
    try:
        config = MessagingConfiguration.load_from_file(path)
        assert config.messaging.local is not None
        assert config.messaging.iot_core is None  # single topology selected at config parse

        provider, connected = _provider_capturing_connects(config, monkeypatch)
        try:
            assert [name for name, _ in connected] == ["local"]
            assert provider._iot_core.client is None  # no IoT Core connection
            assert provider._local.client is not None
        finally:
            provider.disconnect()
    finally:
        os.unlink(path)


def test_dual_broker_topology_when_iot_core_present(monkeypatch):
    """Dual-broker: `messaging.iotCore` present alongside `local` -> the provider connects BOTH
    the local and IoT Core channels (FR-MSG-3)."""
    cfg = {
        "messaging": {
            "local": _local_section(host="emqx.mqtt.svc.cluster.local"),
            "iotCore": _iot_core_section(),
        }
    }
    path = _write_config(cfg)
    try:
        config = MessagingConfiguration.load_from_file(path)
        assert config.messaging.local is not None
        assert config.messaging.iot_core is not None  # dual topology selected at config parse

        provider, connected = _provider_capturing_connects(config, monkeypatch)
        try:
            assert [name for name, _ in connected] == ["local", "iotcore"]
            assert provider._local.client is not None
            assert provider._iot_core.client is not None
        finally:
            provider.disconnect()
    finally:
        os.unlink(path)


# ---------- FR-MSG-3 guard: IoT Core keeps mutual TLS, with NO insecure fallback ----------

def test_iot_core_refuses_to_connect_without_complete_tls_credentials():
    """The IoT Core path requires mutual TLS (caPath+certPath+keyPath); a missing credential must
    raise rather than silently fall back to an unauthenticated/plaintext connection (FR-MSG-3)."""
    config = MessagingConfiguration()
    config.messaging = MessagingConfigData(
        iot_core=IoTCoreConfig(
            endpoint="test.iot.amazonaws.com",
            port=8883,
            client_id="iot-client",
            credentials=CredentialsConfig(cert_path="cert.pem", key_path="key.pem"),  # caPath missing
        )
    )
    provider = StandaloneProvider.__new__(StandaloneProvider)
    with pytest.raises(RuntimeError, match="without complete TLS credentials"):
        provider._configure_tls(MagicMock(), config.messaging.iot_core, "iotcore")