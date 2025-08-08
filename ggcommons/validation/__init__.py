"""
Configuration validation system for ggcommons.

This module provides JSON schema validation for ggcommons configuration
to ensure configuration correctness and provide helpful error messages.
"""

from .configuration_validator import ConfigurationValidator, ConfigurationValidationException

__all__ = [
    'ConfigurationValidator',
    'ConfigurationValidationException'
]