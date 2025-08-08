"""
Service interfaces for ggcommons dependency injection system.

This module provides abstract interfaces that define the contracts for core ggcommons services.
These interfaces enable dependency injection, testing with mocks, and loose coupling between components.
"""

from .i_configuration_service import IConfigurationService
from .i_messaging_service import IMessagingService
from .i_metric_service import IMetricService

__all__ = [
    'IConfigurationService',
    'IMessagingService', 
    'IMetricService'
]