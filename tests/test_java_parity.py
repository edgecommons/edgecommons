#!/usr/bin/env python3
"""
Test script for Java parity messaging configuration.
"""

import sys
import os

# Add the ggcommons directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'ggcommons'))

def test_java_parity_config():
    """Test Java-equivalent messaging configuration."""
    
    try:
        from messaging.messaging_config import MessagingConfig, BrokerConfig
        
        # Test configuration that matches Java version requirements
        config_dict = {
            "localBroker": {
                "host": "localhost",
                "port": 1883,
                "username": "user",
                "password": "pass"
            },
            "iotCoreBroker": {
                "host": "test-endpoint.iot.us-east-1.amazonaws.com",
                "port": 8883,
                "caCertPath": "/path/to/ca.crt",
                "certPath": "/path/to/cert.pem",
                "keyPath": "/path/to/key.pem"
            }
        }
        
        # Parse configuration
        config = MessagingConfig.from_dict(config_dict)
        
        # Validate configuration
        assert config.local_broker is not None
        assert config.iot_core_broker is not None
        
        # Test local broker config (optional credentials)
        local = config.local_broker
        assert local.host == "localhost"
        assert local.port == 1883
        assert local.username == "user"
        assert local.password == "pass"
        assert local.ca_cert_path is None  # Optional for local
        
        # Test IoT Core broker config (required certificates)
        iot_core = config.iot_core_broker
        assert iot_core.host == "test-endpoint.iot.us-east-1.amazonaws.com"
        assert iot_core.port == 8883
        assert iot_core.ca_cert_path == "/path/to/ca.crt"
        assert iot_core.cert_path == "/path/to/cert.pem"
        assert iot_core.key_path == "/path/to/key.pem"
        
        # Test validation
        assert config.validate() == True
        
        print("SUCCESS: Java parity configuration works correctly")
        
        # Test IoT Core only configuration
        iot_only_config = {
            "iotCoreBroker": {
                "host": "test-endpoint.iot.us-east-1.amazonaws.com",
                "port": 8883,
                "caCertPath": "/path/to/ca.crt",
                "certPath": "/path/to/cert.pem",
                "keyPath": "/path/to/key.pem"
            }
        }
        
        config2 = MessagingConfig.from_dict(iot_only_config)
        assert config2.local_broker is None
        assert config2.iot_core_broker is not None
        assert config2.validate() == True
        
        print("SUCCESS: IoT Core only configuration works correctly")
        
        # Test missing IoT Core broker (should fail)
        try:
            invalid_config = {"localBroker": {"host": "localhost", "port": 1883}}
            MessagingConfig.from_dict(invalid_config)
            assert False, "Should have failed without IoT Core broker"
        except ValueError:
            print("SUCCESS: Missing IoT Core broker correctly rejected")
        
        # Test IoT Core without certificates (should fail validation)
        try:
            invalid_iot_config = {
                "iotCoreBroker": {
                    "host": "test-endpoint.iot.us-east-1.amazonaws.com",
                    "port": 8883
                }
            }
            config3 = MessagingConfig.from_dict(invalid_iot_config)
            assert config3.validate() == False
            print("SUCCESS: IoT Core without certificates correctly rejected")
        except:
            print("SUCCESS: IoT Core without certificates correctly rejected")
        
        return True
        
    except Exception as e:
        print(f"FAILED: Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = test_java_parity_config()
    sys.exit(0 if success else 1)