"""
Builder for creating EdgeCommons instances with fluent API.

This module provides a builder pattern for constructing EdgeCommons instances
with improved readability and parameter validation.
"""

import argparse
from typing import Callable, Dict, List, Optional

from edgecommons.config.candidate_validation import (
    DEFAULT_CANDIDATE_VALIDATION_TIMEOUT_SECS,
    ConfigurationCandidateValidator,
    require_validation_timeout,
)


class EdgeCommonsBuilder:
    """
    Builder for creating EdgeCommons instances with fluent API.
    
    Example:
        edgecommons = EdgeCommonsBuilder.create("com.example.MyComponent") \\
            .with_args(args) \\
            .with_app_options(options) \\
            .receive_own_messages(False) \\
            .build()
    """

    def __init__(self, component_name: str):
        """
        Initialize the builder with a component name.
        
        Args:
            component_name: The fully qualified component name
            
        Raises:
            ValueError: If component_name is None or empty
        """
        if not component_name:
            raise ValueError("Component name cannot be None or empty")
            
        self._component_name = component_name
        self._args: Optional[List[str]] = None
        self._app_options: Optional[argparse.ArgumentParser] = None
        self._receive_own_messages = True
        self._initial_ready = True
        self._configuration_validators: Dict[
            str, ConfigurationCandidateValidator
        ] = {}
        self._configuration_validation_timeout = (
            DEFAULT_CANDIDATE_VALIDATION_TIMEOUT_SECS
        )
        self._command_configurers: List[Callable] = []

    @staticmethod
    def create(component_name: str) -> 'EdgeCommonsBuilder':
        """
        Creates a new EdgeCommons builder instance.
        
        Args:
            component_name: The fully qualified component name
            
        Returns:
            A new EdgeCommonsBuilder instance
            
        Raises:
            ValueError: If component_name is None or empty
        """
        return EdgeCommonsBuilder(component_name)

    def with_args(self, args: List[str]) -> 'EdgeCommonsBuilder':
        """
        Sets the command line arguments.
        
        Args:
            args: Command line arguments list
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If args is None
        """
        if args is None:
            raise ValueError("Args cannot be None")
        self._args = args
        return self

    def with_app_options(self, app_options: argparse.ArgumentParser) -> 'EdgeCommonsBuilder':
        """
        Sets custom application options.
        
        Args:
            app_options: Custom ArgumentParser with application-specific options
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If app_options is None
        """
        if app_options is None:
            raise ValueError("App options cannot be None")
        self._app_options = app_options
        return self

    def receive_own_messages(self, receive_own_messages: bool) -> 'EdgeCommonsBuilder':
        """
        Sets whether the component should receive its own messages.
        
        Args:
            receive_own_messages: Flag to determine message reception behavior
            
        Returns:
            This builder instance for method chaining
        """
        self._receive_own_messages = receive_own_messages
        return self

    def initial_ready(self, ready: bool) -> 'EdgeCommonsBuilder':
        """Set the app-controlled readiness flag before any endpoint can start.

        The default is ``True`` for compatibility. Components with mandatory startup gates
        should select ``False`` and call ``gg.set_ready(True)`` only after those gates pass.
        """

        self._initial_ready = bool(ready)
        return self

    def configuration_validator(
        self, name: str, validator: ConfigurationCandidateValidator
    ) -> 'EdgeCommonsBuilder':
        """Register a named pre-commit validator before the config provider starts."""

        if not isinstance(name, str) or not name.strip():
            raise ValueError("configuration validator name must be a non-empty string")
        if not callable(validator):
            raise TypeError("configuration validator must be callable")
        if name in self._configuration_validators:
            raise ValueError(f"configuration validator '{name}' is already registered")
        self._configuration_validators[name] = validator
        return self

    def configuration_validation_timeout(
        self, timeout_secs: float
    ) -> 'EdgeCommonsBuilder':
        """Set the overall validator deadline (default 5 s, hard maximum 60 s)."""

        self._configuration_validation_timeout = require_validation_timeout(timeout_secs)
        return self

    def configure_commands(self, configurer: Callable) -> 'EdgeCommonsBuilder':
        """Configure component handlers before the inbox subscription is acknowledged."""

        if not callable(configurer):
            raise TypeError("command configurer must be callable")
        self._command_configurers.append(configurer)
        return self

    def build(self):
        """
        Builds and returns a configured EdgeCommons instance.
        
        Returns:
            A fully configured EdgeCommons instance
            
        Raises:
            ValueError: If required parameters are missing or invalid
        """
        # Import here to avoid circular imports
        from edgecommons import EdgeCommons
        
        # Use empty args if none provided
        args = self._args if self._args is not None else []
        
        return EdgeCommons(
            component_name=self._component_name,
            args=args,
            app_options=self._app_options,
            receive_own_messages=self._receive_own_messages,
            initial_ready=self._initial_ready,
            configuration_validators=dict(self._configuration_validators),
            configuration_validation_timeout=(
                self._configuration_validation_timeout
            ),
            command_configurers=list(self._command_configurers),
        )
