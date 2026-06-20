"""
Builder for creating Metric instances with fluent API.

This module provides a builder pattern for constructing Metric instances
with improved readability and parameter validation.
"""

from typing import Dict, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from ggcommons.metrics.metric import Metric
    from ggcommons.metrics.measure import Measure


class MetricBuilder:
    """
    Builder for creating Metric instances with fluent API.
    
    Example:
        metric = MetricBuilder.create("cpu_usage") \\
            .with_namespace("MyApp/Metrics") \\
            .add_measure("usage", "Percent", 1) \\
            .add_dimension("instance", "main") \\
            .build()
    """

    def __init__(self, name: str):
        """
        Initialize the builder with a metric name.
        
        Args:
            name: The metric name
            
        Raises:
            ValueError: If name is None or empty
        """
        if not name:
            raise ValueError("Metric name cannot be None or empty")
            
        self._name = name
        self._namespace: Optional[str] = None
        self._thing_name: Optional[str] = None
        self._component_name: Optional[str] = None
        self._measures: Dict[str, 'Measure'] = {}
        self._dimensions: Dict[str, str] = {}

    @staticmethod
    def create(name: str) -> 'MetricBuilder':
        """
        Creates a new Metric builder instance.
        
        Args:
            name: The metric name
            
        Returns:
            A new MetricBuilder instance
            
        Raises:
            ValueError: If name is None or empty
        """
        return MetricBuilder(name)

    def with_namespace(self, namespace: str) -> 'MetricBuilder':
        """
        Sets the metric namespace.
        
        Args:
            namespace: The CloudWatch namespace
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If namespace is None or empty
        """
        if not namespace:
            raise ValueError("Namespace cannot be None or empty")
        self._namespace = namespace
        return self

    def with_thing_name(self, thing_name: str) -> 'MetricBuilder':
        """
        Sets the AWS IoT Thing name.
        
        Args:
            thing_name: The thing name
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If thing_name is None or empty
        """
        if not thing_name:
            raise ValueError("Thing name cannot be None or empty")
        self._thing_name = thing_name
        return self

    def with_component_name(self, component_name: str) -> 'MetricBuilder':
        """
        Sets the component name.
        
        Args:
            component_name: The component name
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If component_name is None or empty
        """
        if not component_name:
            raise ValueError("Component name cannot be None or empty")
        self._component_name = component_name
        return self
        
    def with_config(self, config_service) -> 'MetricBuilder':
        """
        Sets thing_name and component_name from configuration service.
        
        Args:
            config_service: The configuration service
            
        Returns:
            This builder instance for method chaining
        """
        if config_service and hasattr(config_service, 'get_thing_name'):
            self._thing_name = config_service.get_thing_name()
        if config_service and hasattr(config_service, 'get_component_name'):
            self._component_name = config_service.get_component_name()
        return self

    def add_measure(self, name: str, unit: str, storage_resolution: int = 60) -> 'MetricBuilder':
        """
        Adds a measure to the metric.
        
        Args:
            name: The measure name
            unit: The CloudWatch unit (e.g., "Count", "Bytes", "Percent")
            storage_resolution: Storage resolution in seconds (1 or 60)
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If parameters are invalid
        """
        if not name:
            raise ValueError("Measure name cannot be None or empty")
        if not unit:
            raise ValueError("Unit cannot be None or empty")
        if storage_resolution not in [1, 60]:
            raise ValueError("Storage resolution must be 1 or 60 seconds")
            
        # Import here to avoid circular imports
        from ggcommons.metrics.measure import Measure
        
        measure = Measure(name, unit, storage_resolution)
        self._measures[name] = measure
        return self

    def add_dimension(self, key: str, value: str) -> 'MetricBuilder':
        """
        Adds a custom dimension to the metric.
        
        Args:
            key: The dimension key
            value: The dimension value
            
        Returns:
            This builder instance for method chaining
            
        Raises:
            ValueError: If key or value is None or empty
        """
        if not key:
            raise ValueError("Dimension key cannot be None or empty")
        if not value:
            raise ValueError("Dimension value cannot be None or empty")
            
        self._dimensions[key] = value
        return self

    def build(self) -> 'Metric':
        """
        Builds and returns a configured Metric instance.
        
        Returns:
            A fully configured Metric instance
            
        Raises:
            ValueError: If required parameters are missing
        """
        # Import here to avoid circular imports
        from ggcommons.metrics.metric import Metric

        # Build through the real constructor (the sanctioned path: _from_builder=True
        # suppresses the direct-construction deprecation warning). The constructor
        # copies the dicts and injects the default coreName/category/component
        # dimensions.
        metric = Metric(
            self._thing_name or "test-thing",
            self._component_name or "test-component",
            self._name,
            namespace=self._namespace or "GGCommons/Metrics",
            measures=self._measures,
            dimensions=self._dimensions,
            _from_builder=True,
        )

        # Enforce the CloudWatch 10-dimension cap on the assembled total (user
        # dimensions plus the three defaults added by the constructor).
        if len(metric.dimensions) > Metric.MAX_DIMENSIONS:
            raise ValueError(
                f"A metric may have at most {Metric.MAX_DIMENSIONS} dimensions "
                f"(including the default coreName/category/component); got "
                f"{len(metric.dimensions)}"
            )

        return metric