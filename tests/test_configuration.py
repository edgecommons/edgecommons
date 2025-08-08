"""
Unit tests for configuration system.
"""

import unittest
from unittest.mock import Mock, patch, MagicMock
import tempfile
import json
import os

try:
    from ggcommons.validation.configuration_validator import ConfigurationValidator, ConfigurationValidationException
except ImportError:
    import unittest
    raise unittest.SkipTest("AWS SDK dependencies not available")


class TestConfigurationValidator(unittest.TestCase):
    """Test ConfigurationValidator class."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.valid_config = {
            "logging": {
                "level": "INFO",
                "format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s"
            },
            "heartbeat": {
                "intervalSecs": 30,
                "measures": {
                    "cpu": True,
                    "memory": True
                },
                "targets": [{"type": "metric"}]
            },
            "metricEmission": {
                "target": "cloudwatch",
                "namespace": "TestApp"
            },
            "tags": {
                "environment": "test"
            },
            "component": {
                "global": {
                    "setting": "value"
                },
                "instances": [
                    {"id": "main"}
                ]
            }
        }
    
    def test_validate_valid_config(self):
        """Test validation of valid configuration."""
        try:
            ConfigurationValidator.validate(self.valid_config)
        except ConfigurationValidationException:
            self.fail("Valid configuration should not raise exception")
    
    def test_validate_invalid_logging_level(self):
        """Test validation with invalid logging level."""
        invalid_config = self.valid_config.copy()
        invalid_config["logging"]["level"] = "INVALID"
        
        with self.assertRaises(ConfigurationValidationException) as context:
            ConfigurationValidator.validate(invalid_config)
        
        self.assertTrue(len(context.exception.validation_errors) > 0)
    
    def test_validate_invalid_heartbeat_interval(self):
        """Test validation with invalid heartbeat interval."""
        invalid_config = self.valid_config.copy()
        invalid_config["heartbeat"]["intervalSecs"] = 0
        
        with self.assertRaises(ConfigurationValidationException) as context:
            ConfigurationValidator.validate(invalid_config)
        
        self.assertTrue(len(context.exception.validation_errors) > 0)
    
    def test_validate_missing_instance_id(self):
        """Test validation with missing instance ID."""
        invalid_config = self.valid_config.copy()
        invalid_config["component"]["instances"] = [{}]  # Missing id
        
        with self.assertRaises(ConfigurationValidationException) as context:
            ConfigurationValidator.validate(invalid_config)
        
        self.assertTrue(len(context.exception.validation_errors) > 0)
    
    def test_validate_none_config(self):
        """Test validation with None configuration."""
        with self.assertRaises(ValueError):
            ConfigurationValidator.validate(None)
    
    def test_validate_empty_config(self):
        """Test validation with empty configuration."""
        try:
            ConfigurationValidator.validate({})
        except ConfigurationValidationException:
            self.fail("Empty configuration should be valid")
    
    def test_validation_error_details(self):
        """Test that validation errors contain useful details."""
        invalid_config = {
            "logging": {
                "level": "INVALID_LEVEL"
            },
            "heartbeat": {
                "intervalSecs": -1
            }
        }
        
        with self.assertRaises(ConfigurationValidationException) as context:
            ConfigurationValidator.validate(invalid_config)
        
        errors = context.exception.validation_errors
        self.assertTrue(len(errors) >= 2)
        
        # Check that errors contain path and message
        for error in errors:
            self.assertIn('path', error)
            self.assertIn('message', error)


class TestConfigurationValidationException(unittest.TestCase):
    """Test ConfigurationValidationException class."""
    
    def test_init_with_errors(self):
        """Test initialization with validation errors."""
        errors = [
            {"path": "logging.level", "message": "Invalid level"},
            {"path": "heartbeat.intervalSecs", "message": "Must be positive"}
        ]
        
        exception = ConfigurationValidationException("Validation failed", errors)
        
        self.assertEqual(exception.validation_errors, errors)
        self.assertEqual(str(exception), "Validation failed")
    
    def test_init_without_errors(self):
        """Test initialization without validation errors."""
        exception = ConfigurationValidationException("Validation failed")
        
        self.assertEqual(exception.validation_errors, [])


if __name__ == '__main__':
    unittest.main()