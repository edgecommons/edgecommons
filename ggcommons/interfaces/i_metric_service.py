"""
Metric service interface for ggcommons.

This interface defines the contract for metric emission services,
providing abstraction for different metric targets (CloudWatch, logs, messaging).
"""

from abc import ABC, abstractmethod
from typing import Dict
from ggcommons.metrics.metric import Metric


class IMetricService(ABC):
    """
    Interface for metric emission services.
    Provides abstraction for different metric targets (CloudWatch, logs, messaging).
    """

    @abstractmethod
    def define_metric(self, metric: Metric) -> None:
        """
        Defines a new metric for emission.
        
        Args:
            metric: The metric definition to register
            
        Raises:
            ValueError: If metric is None or invalid
        """
        pass

    @abstractmethod
    def emit_metric(self, name: str, measure_values: Dict[str, float]) -> None:
        """
        Emits metric values (may be batched).
        
        Args:
            name: The metric name
            measure_values: Dictionary of measure names to values
            
        Raises:
            ValueError: If name is None/empty or measure_values is None/empty
        """
        pass

    @abstractmethod
    def emit_metric_now(self, name: str, measure_values: Dict[str, float]) -> None:
        """
        Immediately emits metric values (bypasses batching).
        
        Args:
            name: The metric name
            measure_values: Dictionary of measure names to values
            
        Raises:
            ValueError: If name is None/empty or measure_values is None/empty
        """
        pass

    @abstractmethod
    def flush_metrics(self) -> None:
        """
        Flushes any pending metric emissions.
        """
        pass

    @abstractmethod
    def shutdown(self) -> None:
        """
        Shuts down the metric service and releases resources.
        """
        pass