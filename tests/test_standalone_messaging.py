#!/usr/bin/env python3
"""
Test script for STANDALONE messaging mode.

This script demonstrates the new dual broker messaging capabilities.
"""

import json
import sys
import time
from ggcommons.ggcommons_builder import GGCommonsBuilder

def test_standalone_messaging():
    """Test STANDALONE messaging mode."""
    
    # Create test configuration
    config = {
        "messaging": {
            "mode": "STANDALONE",
            "receiveOwnMessages": False,
            "localBroker": {
                "host": "localhost",
                "port": 1883,
                "useTls": False
            },
            "iotCoreBroker": {
                "host": "localhost",
                "port": 8883,
                "useTls": False
            }
        },
        "logging": {
            "level": "INFO"
        }
    }
    
    # Write test config
    with open("test_standalone_config.json", "w") as f:
        json.dump(config, f, indent=2)
    
    try:
        # Initialize GGCommons with STANDALONE mode
        args = ["--mode", "STANDALONE", "-c", "FILE", "test_standalone_config.json", "-t", "test-thing"]
        
        ggcommons = GGCommonsBuilder.create("com.test.StandaloneTest") \
            .with_args(args) \
            .build()
        
        print("✓ GGCommons initialized successfully with STANDALONE mode")
        
        # Get messaging service
        from ggcommons.interfaces import IMessagingService
        messaging = ggcommons.get_service(IMessagingService)
        
        if messaging:
            print("✓ Messaging service available")
        else:
            print("✗ Messaging service not available")
            return False
        
        print("✓ STANDALONE messaging test completed successfully")
        return True
        
    except Exception as e:
        print(f"✗ Test failed: {e}")
        return False
    finally:
        # Cleanup
        import os
        if os.path.exists("test_standalone_config.json"):
            os.remove("test_standalone_config.json")

if __name__ == "__main__":
    success = test_standalone_messaging()
    sys.exit(0 if success else 1)