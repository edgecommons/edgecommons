"""
Configuration validator for edgecommons.

This module validates edgecommons configuration against JSON schema
to ensure configuration correctness and provide helpful error messages.
"""

import json
import logging
from typing import Dict, Any, Optional
from pathlib import Path

try:
    from jsonschema import validate, ValidationError
    JSONSCHEMA_AVAILABLE = True
except ImportError:
    JSONSCHEMA_AVAILABLE = False

logger = logging.getLogger(__name__)


class ConfigurationValidationException(Exception):
    """
    Exception thrown when configuration validation fails.
    """
    
    def __init__(self, message: str, validation_errors: Optional[list] = None):
        """
        Initialize the exception with a message and optional validation errors.
        
        Args:
            message: The error message
            validation_errors: List of validation error details
        """
        super().__init__(message)
        self.validation_errors = validation_errors or []


class ConfigurationValidator:
    """
    Validates edgecommons configuration against JSON schema.
    """
    
    _schema: Optional[Dict[str, Any]] = None
    _schema_loaded = False
    
    @classmethod
    def _load_schema(cls) -> Optional[Dict[str, Any]]:
        """
        Loads the configuration schema from the resources directory.
        
        Returns:
            The loaded schema dictionary or None if not available
        """
        if cls._schema_loaded:
            return cls._schema
            
        cls._schema_loaded = True
        
        if not JSONSCHEMA_AVAILABLE:
            logger.warning("jsonschema library not available, configuration validation disabled")
            return None
            
        try:
            # Primary: load as packaged data so it is found when installed as a
            # wheel (the schema is the parity contract, so silent-disable is bad).
            try:
                from importlib.resources import files
                resource = files("edgecommons.resources").joinpath(
                    "edgecommons-config-schema.json"
                )
                if resource.is_file():
                    cls._schema = json.loads(resource.read_text(encoding="utf-8"))
                    logger.debug("Configuration schema loaded from package resources")
                    return cls._schema
            except (ImportError, FileNotFoundError, ModuleNotFoundError):
                pass

            # Fallback: probe relative paths (editable/source checkouts).
            schema_paths = [
                Path(__file__).parent.parent / "resources" / "edgecommons-config-schema.json",
                Path(__file__).parent.parent.parent / "resources" / "edgecommons-config-schema.json",
                Path("resources") / "edgecommons-config-schema.json"
            ]

            for schema_path in schema_paths:
                if schema_path.exists():
                    with open(schema_path, 'r') as f:
                        cls._schema = json.load(f)
                    logger.debug(f"Configuration schema loaded from {schema_path}")
                    return cls._schema

            logger.warning("Configuration schema file not found, validation disabled")
            return None

        except Exception as e:
            logger.warning(f"Failed to load configuration schema: {e}, validation disabled")
            return None
    
    @classmethod
    def validate(cls, config: Dict[str, Any]) -> None:
        """
        Validates configuration against the JSON schema.
        
        Args:
            config: Configuration dictionary to validate
            
        Raises:
            ConfigurationValidationException: If validation fails
            ValueError: If config is None
        """
        if config is None:
            raise ValueError("Configuration cannot be None")
            
        schema = cls._load_schema()
        if schema is None:
            # Fail closed: validation was requested but jsonschema or the packaged schema is
            # missing. Silently skipping would let a packaging mistake disable the cross-language
            # parity contract (Rust/TS embed the schema and structurally cannot self-disable).
            # 'jsonschema' is a declared dependency, so this only fires on a real packaging fault.
            reason = (
                "the 'jsonschema' library is not installed"
                if not JSONSCHEMA_AVAILABLE
                else "the packaged edgecommons config schema could not be found"
            )
            raise ConfigurationValidationException(
                f"Configuration validation was requested but cannot run: {reason}. "
                "Install dependencies and ship the schema resource, or construct the config "
                "manager with validate_config=False to explicitly opt out of validation."
            )


        try:
            validate(instance=config, schema=schema)
            logger.debug("Configuration validation passed")
            
        except ValidationError as e:
            error_msg = f"Configuration validation failed: {e.message}"
            if e.absolute_path:
                error_msg += f" at path: {'.'.join(str(p) for p in e.absolute_path)}"
                
            validation_errors = [
                {
                    'message': e.message,
                    'path': list(e.absolute_path),
                    'invalid_value': e.instance,
                    'schema_path': list(e.schema_path)
                }
            ]
            
            raise ConfigurationValidationException(error_msg, validation_errors)
            
        except Exception as e:
            raise ConfigurationValidationException(f"Configuration validation error: {str(e)}")
    
    @classmethod
    def validate_section(cls, config_section: Dict[str, Any], section_name: str) -> None:
        """
        Validates a specific configuration section.
        
        Args:
            config_section: Configuration section to validate
            section_name: Name of the section for error reporting
            
        Raises:
            ConfigurationValidationException: If validation fails
            ValueError: If parameters are invalid
        """
        if config_section is None:
            raise ValueError("Configuration section cannot be None")
        if not section_name:
            raise ValueError("Section name cannot be None or empty")
            
        # For section validation, we wrap the section in a full config structure
        full_config = {section_name: config_section}
        
        try:
            cls.validate(full_config)
        except ConfigurationValidationException as e:
            # Re-raise with section-specific context
            raise ConfigurationValidationException(
                f"Validation failed for section '{section_name}': {e}",
                e.validation_errors
            )
    
    @classmethod
    def is_validation_available(cls) -> bool:
        """
        Checks if configuration validation is available.
        
        Returns:
            True if validation is available, False otherwise
        """
        return JSONSCHEMA_AVAILABLE and cls._load_schema() is not None