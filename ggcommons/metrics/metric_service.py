"""
Metric service implementation.

This module provides a concrete implementation of IMetricService
that wraps the existing MetricEmitter functionality.
"""

from typing import Dict, TYPE_CHECKING
from ggcommons.interfaces.i_metric_service import IMetricService
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.metric_emitter import MetricEmitter

if TYPE_CHECKING:
    from ggcommons.config.manager.config_manager import ConfigManager


class MetricService(IMetricService):
    """
    Service implementation that wraps MetricEmitter to provide the IMetricService interface.
    This allows for dependency injection while maintaining backward compatibility.
    """

    def __init__(self, config_manager: 'ConfigManager'):
        """
        Initialize the metric service with a config manager.
        
        Args:
            config_manager: The configuration manager instance
            
        Raises:
            ValueError: If config_manager is None
        """
        if config_manager is None:
            raise ValueError("Config manager cannot be None")
        self._config_manager = config_manager

    def define_metric(self, metric: Metric) -> None:
        """
        Defines a new metric for emission.
        
        Args:
            metric: The metric definition to register
            
        Raises:
            ValueError: If metric is None or invalid
        """
        if metric is None:
            raise ValueError("Metric cannot be None")
        if not hasattr(metric, 'name') or not metric.name:
            raise ValueError("Metric must have a valid name")
            
        MetricEmitter.define_metric(metric)

    def emit_metric(self, name: str, measure_values: Dict[str, float]) -> None:
        """
        Emits metric values (may be batched).
        
        Args:
            name: The metric name
            measure_values: Dictionary of measure names to values
            
        Raises:
            ValueError: If name is None/empty or measure_values is None/empty
        """
        if not name:
            raise ValueError("Metric name cannot be None or empty")
        if not measure_values:
            raise ValueError("Measure values cannot be None or empty")
        
        # Validate all values are numeric
        for measure_name, value in measure_values.items():
            if not isinstance(value, (int, float)):
                raise ValueError(f"Measure value for '{measure_name}' must be numeric, got {type(value)}")
                
        MetricEmitter.emit_metric(name, measure_values)

    def emit_metric_now(self, name: str, measure_values: Dict[str, float]) -> None:
        """
        Immediately emits metric values (bypasses batching).
        
        Args:
            name: The metric name
            measure_values: Dictionary of measure names to values
            
        Raises:
            ValueError: If name is None/empty or measure_values is None/empty
        """
        if not name:
            raise ValueError("Metric name cannot be None or empty")
        if not measure_values:
            raise ValueError("Measure values cannot be None or empty")
        
        # Validate all values are numeric
        for measure_name, value in measure_values.items():
            if not isinstance(value, (int, float)):
                raise ValueError(f"Measure value for '{measure_name}' must be numeric, got {type(value)}")
                
        MetricEmitter.emit_metric_now(name, measure_values)

    def flush_metrics(self) -> None:
        """
        Flushes any pending metric emissions.
        """
        # MetricEmitter doesn't currently have a flush method, but we can add it later
        pass

    def shutdown(self) -> None:
        """
        Shuts down the metric service and releases resources.
        """
        # MetricEmitter doesn't currently have a shutdown method, but we can add it later
        pass