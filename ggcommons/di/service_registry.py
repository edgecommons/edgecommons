"""
Service registry for dependency injection.

This module provides a simple dependency injection container for managing service instances.
Services are registered by type and can be retrieved by their interface type.
"""

from typing import Dict, Type, TypeVar, Optional, Any
import threading

T = TypeVar('T')


class ServiceRegistry:
    """
    Simple dependency injection container for managing service instances.
    Provides registration and lookup of services by type.
    
    This class is thread-safe and can be used from multiple threads concurrently.
    """

    def __init__(self):
        """Initialize the service registry with an empty service map."""
        self._services: Dict[Type, Any] = {}
        self._lock = threading.RLock()

    def register(self, service_type: Type[T], implementation: T) -> None:
        """
        Registers a service implementation for the given service type.
        
        Args:
            service_type: The service interface or class type
            implementation: The service implementation instance
            
        Raises:
            ValueError: If service_type or implementation is None
        """
        if service_type is None:
            raise ValueError("Service type cannot be None")
        if implementation is None:
            raise ValueError("Implementation cannot be None")
            
        with self._lock:
            self._services[service_type] = implementation

    def get(self, service_type: Type[T]) -> Optional[T]:
        """
        Retrieves a service implementation by type.
        
        Args:
            service_type: The service type to retrieve
            
        Returns:
            The service implementation, or None if not registered
            
        Raises:
            ValueError: If service_type is None
        """
        if service_type is None:
            raise ValueError("Service type cannot be None")
            
        with self._lock:
            return self._services.get(service_type)

    def is_registered(self, service_type: Type) -> bool:
        """
        Checks if a service is registered for the given type.
        
        Args:
            service_type: The service type to check
            
        Returns:
            True if registered, False otherwise
            
        Raises:
            ValueError: If service_type is None
        """
        if service_type is None:
            raise ValueError("Service type cannot be None")
            
        with self._lock:
            return service_type in self._services

    def unregister(self, service_type: Type) -> None:
        """
        Removes a service registration.
        
        Args:
            service_type: The service type to unregister
            
        Raises:
            ValueError: If service_type is None
        """
        if service_type is None:
            raise ValueError("Service type cannot be None")
            
        with self._lock:
            self._services.pop(service_type, None)

    def clear(self) -> None:
        """
        Clears all service registrations.
        """
        with self._lock:
            self._services.clear()

    def get_registered_types(self) -> list:
        """
        Returns a list of all registered service types.
        
        Returns:
            List of registered service types
        """
        with self._lock:
            return list(self._services.keys())