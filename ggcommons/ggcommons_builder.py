"""
Builder for creating GGCommons instances with fluent API.

This module provides a builder pattern for constructing GGCommons instances
with improved readability and parameter validation.
"""

import argparse
from typing import Optional, List


class GGCommonsBuilder:
    """
    Builder for creating GGCommons instances with fluent API.
    
    Example:
        ggcommons = GGCommonsBuilder.create("com.example.MyComponent") \\
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

    @staticmethod
    def create(component_name: str) -> 'GGCommonsBuilder':
        """
        Creates a new GGCommons builder instance.
        
        Args:
            component_name: The fully qualified component name
            
        Returns:
            A new GGCommonsBuilder instance
            
        Raises:
            ValueError: If component_name is None or empty
        """
        return GGCommonsBuilder(component_name)

    def with_args(self, args: List[str]) -> 'GGCommonsBuilder':
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

    def with_app_options(self, app_options: argparse.ArgumentParser) -> 'GGCommonsBuilder':
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

    def receive_own_messages(self, receive_own_messages: bool) -> 'GGCommonsBuilder':
        """
        Sets whether the component should receive its own messages.
        
        Args:
            receive_own_messages: Flag to determine message reception behavior
            
        Returns:
            This builder instance for method chaining
        """
        self._receive_own_messages = receive_own_messages
        return self

    def build(self):
        """
        Builds and returns a configured GGCommons instance.
        
        Returns:
            A fully configured GGCommons instance
            
        Raises:
            ValueError: If required parameters are missing or invalid
        """
        # Import here to avoid circular imports
        from ggcommons import GGCommons
        
        # Use empty args if none provided
        args = self._args if self._args is not None else []
        
        return GGCommons(
            component_name=self._component_name,
            args=args,
            app_options=self._app_options,
            receive_own_messages=self._receive_own_messages
        )