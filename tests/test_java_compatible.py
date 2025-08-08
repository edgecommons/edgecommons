#!/usr/bin/env python3
"""
Test script for Java-compatible messaging configuration.
"""

import sys
import os
import json

# Add the parent directory to the path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..'))

def test_java_compatible_config():
    """Test Java-compatible messaging configuration."""
    
    try:
        from ggcommons.messaging.messaging_config import MessagingConfiguration
        
        # Create Java-compatible configuration file
        java_config = {
            "messaging": {
                "local": {
                    "type": "mqtt",
                    "host": "localhost",
                    "port": 1883,
                    "clientId": "my-component-local",
                    "credentials": {
                        "username": "testuser",
                        "password": "testpass"
                    }
                },
                "iotCore": {
                    "endpoint": "a2djc899idbttw-ats.iot.us-east-1.amazonaws.com",
                    "port": 8883,
                    "clientId": "ggcommons-test-2",
                    "credentials": {
                        "certPath": "creds/ggcommons-test-2.cert.pem",
                        "keyPath": "creds/ggcommons-test-2.private.key",
                        "caPath": "creds/root-CA.crt"
                    }
                }
            }
        }
        
        # Write test config file
        config_file = "test_java_config.json"
        with open(config_file, 'w') as f:
            json.dump(java_config, f, indent=2)
        
        try:
            # Load configuration
            config = MessagingConfiguration.load_from_file(config_file)
            
            # Validate structure matches Java version
            assert config.messaging is not None
            assert config.messaging.local is not None
            assert config.messaging.iot_core is not None
            
            # Test local broker config
            local = config.messaging.local
            assert local.type == "mqtt"
            assert local.host == "localhost"
            assert local.port == 1883
            assert local.client_id == "my-component-local"
            assert local.credentials is not None
            assert local.credentials.username == "testuser"
            assert local.credentials.password == "testpass"
            
            # Test IoT Core config
            iot_core = config.messaging.iot_core
            assert iot_core.endpoint == "a2djc899idbttw-ats.iot.us-east-1.amazonaws.com"
            assert iot_core.port == 8883
            assert iot_core.client_id == "ggcommons-test-2"
            assert iot_core.credentials is not None
            assert iot_core.credentials.cert_path == "creds/ggcommons-test-2.cert.pem"
            assert iot_core.credentials.key_path == "creds/ggcommons-test-2.private.key"
            assert iot_core.credentials.ca_path == "creds/root-CA.crt"
            
            # Test validation
            assert config.validate() == True
            
            print("SUCCESS: Java-compatible configuration parsing works correctly")
            
            # Test IoT Core only configuration (local is optional)
            iot_only_config = {
                "messaging": {
                    "iotCore": {
                        "endpoint": "test-endpoint.iot.us-east-1.amazonaws.com",
                        "port": 8883,
                        "clientId": "test-client",
                        "credentials": {
                            "certPath": "test.cert.pem",
                            "keyPath": "test.private.key",
                            "caPath": "root-CA.crt"
                        }
                    }
                }
            }
            
            iot_config_file = "test_iot_only_config.json"
            with open(iot_config_file, 'w') as f:
                json.dump(iot_only_config, f, indent=2)
            
            try:
                config2 = MessagingConfiguration.load_from_file(iot_config_file)
                assert config2.messaging.local is None
                assert config2.messaging.iot_core is not None
                assert config2.validate() == True
                
                print("SUCCESS: IoT Core only configuration works correctly")
                
            finally:
                if os.path.exists(iot_config_file):
                    os.remove(iot_config_file)
            
            # Test missing IoT Core (should fail)
            try:
                invalid_config = {"messaging": {"local": {"type": "mqtt", "host": "localhost", "port": 1883, "clientId": "test"}}}
                invalid_config_file = "test_invalid_config.json"
                with open(invalid_config_file, 'w') as f:
                    json.dump(invalid_config, f, indent=2)
                
                try:
                    MessagingConfiguration.load_from_file(invalid_config_file)
                    assert False, "Should have failed without IoT Core config"
                except ValueError:
                    print("SUCCESS: Missing IoT Core configuration correctly rejected")
                finally:
                    if os.path.exists(invalid_config_file):
                        os.remove(invalid_config_file)
            except Exception as e:
                print(f"SUCCESS: Missing IoT Core configuration correctly rejected: {e}")
            
            return True
            
        finally:
            if os.path.exists(config_file):
                os.remove(config_file)
        
    except Exception as e:
        print(f"FAILED: Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

if __name__ == "__main__":
    success = test_java_compatible_config()
    sys.exit(0 if success else 1)