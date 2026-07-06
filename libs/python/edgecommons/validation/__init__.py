"""
Configuration validation system for edgecommons.

This module provides JSON schema validation for edgecommons configuration
to ensure configuration correctness and provide helpful error messages.
"""

from .configuration_validator import ConfigurationValidator, ConfigurationValidationException

__all__ = [
    'ConfigurationValidator',
    'ConfigurationValidationException'
]