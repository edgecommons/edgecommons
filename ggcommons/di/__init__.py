"""
Dependency injection system for ggcommons.

This module provides a simple dependency injection container for managing service instances.
It enables loose coupling between components and facilitates testing with mock services.
"""

from .service_registry import ServiceRegistry
from .service_factory import ServiceFactory

__all__ = [
    'ServiceRegistry',
    'ServiceFactory'
]