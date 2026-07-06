"""
Messaging configuration classes matching Java version exactly.

This module provides configuration classes for the messaging system
that matches the Java implementation schema and behavior.
"""

from dataclasses import dataclass, field
from typing import Optional, Any
import logging
import json

logger = logging.getLogger(__name__)


@dataclass
class CredentialsConfig:
    """Configuration for MQTT broker credentials."""
    
    username: Optional[str] = None
    password: Optional[str] = None
    cert_path: Optional[str] = None
    key_path: Optional[str] = None
    ca_path: Optional[str] = None


@dataclass
class QosDefaults:
    """Default MQTT QoS for publish/subscribe operations without an explicit QoS."""

    publish: int = 1
    subscribe: int = 1


@dataclass
class LocalMqttConfig:
    """Configuration for local MQTT broker."""

    type: str
    host: str
    port: int
    client_id: str
    qos: QosDefaults = field(default_factory=QosDefaults)
    credentials: Optional[CredentialsConfig] = None


@dataclass
class NorthboundMqttConfig:
    """Configuration for the optional generic northbound MQTT broker."""

    endpoint: str
    port: int
    client_id: str
    qos: QosDefaults = field(default_factory=QosDefaults)
    credentials: Optional[CredentialsConfig] = None

    @property
    def host(self) -> str:
        return self.endpoint


def _parse_qos_value(section: dict, key: str, max_value: int, field: str) -> int:
    raw = section.get(key, 1)
    if isinstance(raw, float) and raw.is_integer():
        raw = int(raw)
    if not isinstance(raw, int) or raw < 0 or raw > max_value:
        raise ValueError(f"{field} must be 0..{max_value} (got {raw})")
    return raw


def _parse_qos_defaults(raw: Any, max_value: int, prefix: str) -> QosDefaults:
    section = raw if isinstance(raw, dict) else {}
    return QosDefaults(
        publish=_parse_qos_value(section, "publish", max_value, f"{prefix}.publish"),
        subscribe=_parse_qos_value(section, "subscribe", max_value, f"{prefix}.subscribe"),
    )


@dataclass
class MessagingConfigData:
    """Inner messaging configuration data."""

    local: Optional[LocalMqttConfig] = None
    northbound: Optional[NorthboundMqttConfig] = None


class MessagingConfiguration:
    """Configuration class for standalone messaging setup."""
    
    def __init__(self):
        self.messaging: Optional[MessagingConfigData] = None
    
    @classmethod
    def load_from_file(cls, config_path: str) -> 'MessagingConfiguration':
        """Load messaging configuration from file."""
        logger.info(f"Loading messaging configuration from file: {config_path}")
        
        try:
            logger.debug(f"Reading configuration file: {config_path}")
            with open(config_path, 'r') as f:
                data = json.load(f)
            
            logger.debug("Successfully parsed JSON from configuration file")
            config = cls()
            messaging_data = data.get('messaging', {})
            
            if not messaging_data:
                logger.warning("No 'messaging' section found in configuration file")

            if 'lwt' in messaging_data:
                raise ValueError(
                    "messaging.lwt is not supported; uns-bridge derives its site Last-Will internally"
                )
            if 'qos' in messaging_data:
                raise ValueError(
                    "messaging.qos is not supported; configure QoS under messaging.local.qos and messaging.northbound.qos"
                )
            
            # Parse local broker config
            local_config = None
            if 'local' in messaging_data:
                logger.debug("Parsing local broker configuration")
                local_data = messaging_data['local']
                
                credentials = None
                if 'credentials' in local_data:
                    logger.debug("Parsing local broker credentials")
                    cred_data = local_data['credentials']
                    credentials = CredentialsConfig(
                        username=cred_data.get('username'),
                        password=cred_data.get('password'),
                        cert_path=cred_data.get('certPath'),
                        key_path=cred_data.get('keyPath'),
                        ca_path=cred_data.get('caPath')
                    )
                    
                    auth_methods = []
                    if credentials.username and credentials.password:
                        auth_methods.append("username/password")
                    if credentials.cert_path and credentials.key_path:
                        auth_methods.append("certificate")
                    logger.debug(f"Local broker authentication methods: {', '.join(auth_methods) if auth_methods else 'none'}")
                
                local_config = LocalMqttConfig(
                    # `type` is an unvalidated broker tag (no schema enum) that nothing consumes; it is
                    # set by the Java/Python samples and ignored by Rust/TS. Default it to "mqtt" so a
                    # config without it is accepted (parity with Rust/TS, which don't require it).
                    type=local_data.get('type', 'mqtt'),
                    host=local_data['host'],
                    port=local_data['port'],
                    client_id=local_data['clientId'],
                    qos=_parse_qos_defaults(local_data.get('qos'), 2, "messaging.local.qos"),
                    credentials=credentials
                )
                
                logger.info(f"Local broker configured: {local_config.host}:{local_config.port} (client_id: {local_config.client_id})")
            else:
                logger.info("No local broker configuration found")
            
            # Parse generic northbound broker config (optional). It is a normal
            # MQTT broker: plaintext, username/password, server TLS, or mutual TLS
            # are selected from the credentials block the same way as local.
            northbound_config = None
            if 'northbound' in messaging_data:
                logger.debug("Parsing northbound broker configuration")
                northbound_data = messaging_data['northbound']

                northbound_credentials = None
                if 'credentials' in northbound_data:
                    cred_data = northbound_data['credentials']
                    northbound_credentials = CredentialsConfig(
                        username=cred_data.get('username'),
                        password=cred_data.get('password'),
                        cert_path=cred_data.get('certPath'),
                        key_path=cred_data.get('keyPath'),
                        ca_path=cred_data.get('caPath')
                    )

                northbound_host = northbound_data.get('host') or northbound_data.get('endpoint')
                northbound_config = NorthboundMqttConfig(
                    endpoint=northbound_host,
                    port=northbound_data['port'],
                    client_id=northbound_data['clientId'],
                    qos=_parse_qos_defaults(northbound_data.get('qos'), 2, "messaging.northbound.qos"),
                    credentials=northbound_credentials
                )

                logger.info(
                    f"Northbound broker configured: {northbound_config.endpoint}:{northbound_config.port} "
                    f"(client_id: {northbound_config.client_id})"
                )
            else:
                logger.info("No northbound broker configuration found (local-only standalone)")

            config.messaging = MessagingConfigData(
                local=local_config,
                northbound=northbound_config
            )
            
            logger.info(f"Successfully loaded messaging configuration from {config_path}")
            return config
            
        except FileNotFoundError:
            logger.error(f"Messaging configuration file not found: {config_path}")
            raise
        except json.JSONDecodeError as e:
            logger.error(f"Invalid JSON in messaging configuration file {config_path}: {e}")
            raise
        except KeyError as e:
            logger.error(f"Missing required configuration key in {config_path}: {e}")
            raise
        except Exception as e:
            logger.error(f"Failed to load messaging configuration from {config_path}: {e}")
            raise
    
    def validate(self) -> bool:
        """Validate messaging configuration."""
        logger.debug("Validating messaging configuration")
        
        if not self.messaging:
            logger.error("Messaging configuration is required but not provided")
            return False

        # At least one broker (local or northbound) must be configured.
        if not self.messaging.local and not self.messaging.northbound:
            logger.error("At least one of 'local' or 'northbound' must be configured")
            return False

        if self.messaging.northbound:
            logger.debug("Validating northbound broker configuration")
            if not self.messaging.northbound.endpoint or not self.messaging.northbound.port:
                logger.error("Northbound broker requires host/endpoint and port")
                return False

        # Validate local broker if configured
        if self.messaging.local:
            logger.debug("Validating local broker configuration")
            local = self.messaging.local
            if not local.host or not local.port:
                logger.error("Local broker requires host and port")
                return False
        
        logger.info("Messaging configuration validation passed")
        return True
