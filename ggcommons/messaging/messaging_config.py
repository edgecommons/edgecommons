"""
Messaging configuration classes matching Java version exactly.

This module provides configuration classes for the messaging system
that matches the Java implementation schema and behavior.
"""

from dataclasses import dataclass
from typing import Optional, Dict, Any
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
class LocalMqttConfig:
    """Configuration for local MQTT broker."""
    
    type: str
    host: str
    port: int
    client_id: str
    credentials: Optional[CredentialsConfig] = None


@dataclass
class IoTCoreConfig:
    """Configuration for IoT Core broker."""
    
    endpoint: str
    port: int
    client_id: str
    credentials: CredentialsConfig


@dataclass
class MessagingConfigData:
    """Inner messaging configuration data."""
    
    local: Optional[LocalMqttConfig] = None
    iot_core: Optional[IoTCoreConfig] = None


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
            
            logger.debug(f"Successfully parsed JSON from configuration file")
            config = cls()
            messaging_data = data.get('messaging', {})
            
            if not messaging_data:
                logger.warning("No 'messaging' section found in configuration file")
            
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
                    type=local_data['type'],
                    host=local_data['host'],
                    port=local_data['port'],
                    client_id=local_data['clientId'],
                    credentials=credentials
                )
                
                logger.info(f"Local broker configured: {local_config.host}:{local_config.port} (client_id: {local_config.client_id})")
            else:
                logger.info("No local broker configuration found")
            
            # Parse IoT Core config (optional — parity with Java/Rust, which allow a
            # local-only standalone deployment). When present it must carry creds.
            iot_config = None
            if 'iotCore' in messaging_data:
                logger.debug("Parsing IoT Core broker configuration")
                iot_data = messaging_data['iotCore']

                if 'credentials' not in iot_data:
                    logger.error("IoT Core credentials are required but not found")
                    raise ValueError("IoT Core credentials are required")

                iot_credentials = CredentialsConfig(
                    cert_path=iot_data['credentials']['certPath'],
                    key_path=iot_data['credentials']['keyPath'],
                    ca_path=iot_data['credentials']['caPath']
                )

                iot_config = IoTCoreConfig(
                    endpoint=iot_data['endpoint'],
                    port=iot_data['port'],
                    client_id=iot_data['clientId'],
                    credentials=iot_credentials
                )

                logger.info(f"IoT Core broker configured: {iot_config.endpoint}:{iot_config.port} (client_id: {iot_config.client_id})")
                logger.debug(f"IoT Core certificate paths - CA: {iot_credentials.ca_path}, Cert: {iot_credentials.cert_path}, Key: {iot_credentials.key_path}")
            else:
                logger.info("No IoT Core configuration found (local-only standalone)")

            config.messaging = MessagingConfigData(
                local=local_config,
                iot_core=iot_config
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

        # At least one broker (local or IoT Core) must be configured.
        if not self.messaging.local and not self.messaging.iot_core:
            logger.error("At least one of 'local' or 'iotCore' must be configured")
            return False

        # IoT Core, when configured, must carry complete certificate credentials.
        if self.messaging.iot_core:
            logger.debug("Validating IoT Core configuration")
            iot_creds = self.messaging.iot_core.credentials
            if not iot_creds:
                logger.error("IoT Core credentials are required but not provided")
                return False

            missing_creds = []
            if not iot_creds.cert_path:
                missing_creds.append("certificate path")
            if not iot_creds.key_path:
                missing_creds.append("private key path")
            if not iot_creds.ca_path:
                missing_creds.append("CA certificate path")

            if missing_creds:
                logger.error(f"IoT Core missing required credentials: {', '.join(missing_creds)}")
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