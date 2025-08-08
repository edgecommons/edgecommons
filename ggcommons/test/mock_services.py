"""
Mock service implementations for testing.

This module provides mock implementations of all ggcommons service interfaces
for use in unit testing and integration testing scenarios.
"""

from typing import Dict, Any, Optional, Collection, List, Callable
from concurrent.futures import Future, CompletedFuture
from awsiot.greengrasscoreipc.model import QOS
from ggcommons.interfaces import IConfigurationService, IMessagingService, IMetricService
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.metrics.metric import Metric


class MockConfigurationService(IConfigurationService):
    """
    Mock configuration service for testing.
    
    Provides controllable configuration data and change notification testing.
    """
    
    def __init__(self, initial_config: Optional[Dict[str, Any]] = None):
        """
        Initialize mock configuration service.
        
        Args:
            initial_config: Initial configuration data
        """
        self._config = initial_config or {
            'component': {
                'global': {},
                'instances': []
            }
        }
        self._thing_name = "test-thing"
        self._component_name = "test-component"
        self._component_full_name = "com.test.TestComponent"
        self._listeners: List[ConfigurationChangeListener] = []
        
    def get_global_config(self) -> Dict[str, Any]:
        """Get global configuration."""
        return self._config.get('component', {}).get('global', {})
        
    def get_instance_config(self, instance_id: str) -> Optional[Dict[str, Any]]:
        """Get instance configuration."""
        if not instance_id:
            raise ValueError("Instance ID cannot be None or empty")
            
        instances = self._config.get('component', {}).get('instances', [])
        for instance in instances:
            if instance.get('id') == instance_id:
                return instance
        return None
        
    def get_instance_ids(self) -> Collection[str]:
        """Get all instance IDs."""
        instances = self._config.get('component', {}).get('instances', [])
        return [instance.get('id') for instance in instances if 'id' in instance]
        
    def get_full_config(self) -> Dict[str, Any]:
        """Get full configuration."""
        return self._config.copy()
        
    def get_thing_name(self) -> Optional[str]:
        """Get thing name."""
        return self._thing_name
        
    def get_component_name(self) -> str:
        """Get component name."""
        return self._component_name
        
    def get_component_full_name(self) -> str:
        """Get full component name."""
        return self._component_full_name
        
    def resolve_template(self, template: str) -> str:
        """Resolve template variables."""
        if template is None:
            raise ValueError("Template cannot be None")
            
        resolved = template
        resolved = resolved.replace('{ThingName}', self._thing_name or 'unknown')
        resolved = resolved.replace('{ComponentName}', self._component_name)
        resolved = resolved.replace('{ComponentFullName}', self._component_full_name)
        
        # Resolve tag variables
        tags = self._config.get('tags', {})
        for key, value in tags.items():
            resolved = resolved.replace(f'{{{key}}}', str(value))
            
        return resolved
        
    def add_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """Add configuration change listener."""
        if listener is None:
            raise ValueError("Listener cannot be None")
        self._listeners.append(listener)
        
    def remove_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """Remove configuration change listener."""
        if listener is None:
            raise ValueError("Listener cannot be None")
        if listener in self._listeners:
            self._listeners.remove(listener)
            
    def notify_configuration_changed(self) -> None:
        """Notify all listeners of configuration change."""
        for listener in self._listeners:
            try:
                listener.on_configuration_change(self._config)
            except Exception:
                pass  # Ignore listener errors in mock
                
    # Test helper methods
    def set_config(self, config: Dict[str, Any]) -> None:
        """Set configuration for testing."""
        self._config = config
        
    def set_thing_name(self, thing_name: str) -> None:
        """Set thing name for testing."""
        self._thing_name = thing_name
        
    def set_component_name(self, component_name: str) -> None:
        """Set component name for testing."""
        self._component_name = component_name
        
    def trigger_config_change(self, new_config: Dict[str, Any]) -> None:
        """Trigger configuration change for testing."""
        self._config = new_config
        self.notify_configuration_changed()


class PublishedMessage:
    """Represents a published message for verification in tests."""
    
    def __init__(self, topic: str, message: Any, qos: Optional[QOS] = None, destination: str = 'ipc'):
        self.topic = topic
        self.message = message
        self.qos = qos
        self.destination = destination
        self.timestamp = None  # Could add timestamp if needed


class MockMessagingService(IMessagingService):
    """
    Mock messaging service for testing.
    
    Captures published messages and allows controlled message injection for testing.
    """
    
    def __init__(self):
        """Initialize mock messaging service."""
        self.published_messages: List[PublishedMessage] = []
        self.subscriptions: Dict[str, List[Callable]] = {}
        self.request_responses: Dict[str, Any] = {}
        
    def subscribe(self, topic: str, handler: Callable[[str, Any], None], max_messages: int = 10) -> None:
        """Subscribe to messages."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
        if max_messages <= 0:
            raise ValueError("Max messages must be positive")
            
        if topic not in self.subscriptions:
            self.subscriptions[topic] = []
        self.subscriptions[topic].append(handler)
        
    def subscribe_to_iot_core(self, topic: str, handler: Callable[[str, Any], None], 
                             qos: QOS, max_messages: int = 10) -> None:
        """Subscribe to IoT Core messages."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if handler is None:
            raise ValueError("Handler cannot be None")
        if qos is None:
            raise ValueError("QOS cannot be None")
        if max_messages <= 0:
            raise ValueError("Max messages must be positive")
            
        # Store with special marker for IoT Core
        iot_topic = f"iot_core:{topic}"
        if iot_topic not in self.subscriptions:
            self.subscriptions[iot_topic] = []
        self.subscriptions[iot_topic].append(handler)
        
    def publish(self, topic: str, message: Any) -> None:
        """Publish message via IPC."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        self.published_messages.append(PublishedMessage(topic, message, destination='ipc'))
        
    def publish_to_iot_core(self, topic: str, message: Any, qos: QOS) -> None:
        """Publish message to IoT Core."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
        if qos is None:
            raise ValueError("QOS cannot be None")
            
        self.published_messages.append(PublishedMessage(topic, message, qos, 'iot_core'))
        
    def publish_raw(self, topic: str, payload: Dict[str, Any]) -> None:
        """Publish raw JSON payload."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if payload is None:
            raise ValueError("Payload cannot be None")
            
        self.published_messages.append(PublishedMessage(topic, payload, destination='ipc'))
        
    def request(self, topic: str, message: Any) -> Future:
        """Send request and return future."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        # Return pre-configured response or None
        response = self.request_responses.get(topic)
        return CompletedFuture(response)
        
    def request_from_iot_core(self, topic: str, message: Any) -> Future:
        """Send IoT Core request and return future."""
        if not topic:
            raise ValueError("Topic cannot be None or empty")
        if message is None:
            raise ValueError("Message cannot be None")
            
        response = self.request_responses.get(f"iot_core:{topic}")
        return CompletedFuture(response)
        
    def reply(self, original_message: Any, reply_message: Any) -> None:
        """Send reply to message."""
        if original_message is None:
            raise ValueError("Original message cannot be None")
        if reply_message is None:
            raise ValueError("Reply message cannot be None")
        # Mock implementation - could track replies if needed
        
    def unsubscribe(self, topic_filter: str) -> None:
        """Unsubscribe from topic."""
        if not topic_filter:
            raise ValueError("Topic filter cannot be None or empty")
        if topic_filter in self.subscriptions:
            del self.subscriptions[topic_filter]
            
    def unsubscribe_from_iot_core(self, topic_filter: str) -> None:
        """Unsubscribe from IoT Core topic."""
        if not topic_filter:
            raise ValueError("Topic filter cannot be None or empty")
        iot_topic = f"iot_core:{topic_filter}"
        if iot_topic in self.subscriptions:
            del self.subscriptions[iot_topic]
            
    def topic_matches_filter(self, topic_filter: str, topic: str) -> bool:
        """Check if topic matches filter."""
        if not topic_filter or not topic:
            raise ValueError("Topic filter and topic cannot be None or empty")
        # Simple implementation for testing
        return topic_filter == topic or topic_filter.endswith('+') or topic_filter.endswith('#')
        
    def get_native_local_client(self) -> Optional[Any]:
        """Get native local client."""
        return None  # Mock doesn't have real clients
        
    def get_native_iot_core_client(self) -> Optional[Any]:
        """Get native IoT Core client."""
        return None  # Mock doesn't have real clients
        
    # Test helper methods
    def clear_published_messages(self) -> None:
        """Clear published messages for testing."""
        self.published_messages.clear()
        
    def get_published_messages_for_topic(self, topic: str) -> List[PublishedMessage]:
        """Get published messages for specific topic."""
        return [msg for msg in self.published_messages if msg.topic == topic]
        
    def set_request_response(self, topic: str, response: Any) -> None:
        """Set response for request testing."""
        self.request_responses[topic] = response
        
    def inject_message(self, topic: str, message: Any) -> None:
        """Inject message to subscribers for testing."""
        # Find matching subscriptions
        for sub_topic, handlers in self.subscriptions.items():
            if self.topic_matches_filter(sub_topic, topic):
                for handler in handlers:
                    try:
                        handler(topic, message)
                    except Exception:
                        pass  # Ignore handler errors in mock


class EmittedMetric:
    """Represents an emitted metric for verification in tests."""
    
    def __init__(self, name: str, values: Dict[str, float], immediate: bool = False):
        self.name = name
        self.values = values.copy()
        self.immediate = immediate
        self.timestamp = None  # Could add timestamp if needed


class MockMetricService(IMetricService):
    """
    Mock metric service for testing.
    
    Captures emitted metrics and defined metrics for verification in tests.
    """
    
    def __init__(self):
        """Initialize mock metric service."""
        self.defined_metrics: Dict[str, Metric] = {}
        self.emitted_metrics: List[EmittedMetric] = []
        
    def define_metric(self, metric: Metric) -> None:
        """Define a metric."""
        if metric is None:
            raise ValueError("Metric cannot be None")
        if not hasattr(metric, 'name') or not metric.name:
            raise ValueError("Metric must have a valid name")
            
        self.defined_metrics[metric.name] = metric
        
    def emit_metric(self, name: str, measure_values: Dict[str, float]) -> None:
        """Emit metric values (batched)."""
        if not name:
            raise ValueError("Metric name cannot be None or empty")
        if not measure_values:
            raise ValueError("Measure values cannot be None or empty")
            
        # Validate values are numeric
        for measure_name, value in measure_values.items():
            if not isinstance(value, (int, float)):
                raise ValueError(f"Measure value for '{measure_name}' must be numeric, got {type(value)}")
                
        self.emitted_metrics.append(EmittedMetric(name, measure_values, immediate=False))
        
    def emit_metric_now(self, name: str, measure_values: Dict[str, float]) -> None:
        """Emit metric values immediately."""
        if not name:
            raise ValueError("Metric name cannot be None or empty")
        if not measure_values:
            raise ValueError("Measure values cannot be None or empty")
            
        # Validate values are numeric
        for measure_name, value in measure_values.items():
            if not isinstance(value, (int, float)):
                raise ValueError(f"Measure value for '{measure_name}' must be numeric, got {type(value)}")
                
        self.emitted_metrics.append(EmittedMetric(name, measure_values, immediate=True))
        
    def flush_metrics(self) -> None:
        """Flush pending metrics."""
        pass  # Mock doesn't need to flush
        
    def shutdown(self) -> None:
        """Shutdown metric service."""
        pass  # Mock doesn't need cleanup
        
    # Test helper methods
    def clear_emitted_metrics(self) -> None:
        """Clear emitted metrics for testing."""
        self.emitted_metrics.clear()
        
    def get_emitted_metrics_for_name(self, name: str) -> List[EmittedMetric]:
        """Get emitted metrics for specific name."""
        return [metric for metric in self.emitted_metrics if metric.name == name]
        
    def is_metric_defined(self, name: str) -> bool:
        """Check if metric is defined."""
        return name in self.defined_metrics
        
    def get_defined_metric(self, name: str) -> Optional[Metric]:
        """Get defined metric by name."""
        return self.defined_metrics.get(name)