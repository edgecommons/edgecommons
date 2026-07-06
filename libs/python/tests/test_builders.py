"""
Unit tests for builder classes.
"""

import pytest
from unittest.mock import Mock, patch

try:
    from edgecommons.edgecommons_builder import EdgeCommonsBuilder
    from edgecommons.messaging.message_builder import MessageBuilder
    from edgecommons.metrics.metric_builder import MetricBuilder
except ImportError:
    pytest.skip("AWS SDK dependencies not available", allow_module_level=True)


# EdgeCommonsBuilder tests
def test_edgecommons_builder_create():
    """Test create method."""
    builder = EdgeCommonsBuilder.create("com.test.Component")
    assert isinstance(builder, EdgeCommonsBuilder)
    assert builder._component_name == "com.test.Component"


def test_edgecommons_builder_create_empty_name():
    """Test create with empty component name."""
    with pytest.raises(ValueError):
        EdgeCommonsBuilder.create("")


def test_edgecommons_builder_create_none_name():
    """Test create with None component name."""
    with pytest.raises(ValueError):
        EdgeCommonsBuilder.create(None)


def test_edgecommons_builder_with_args():
    """Test with_args method."""
    builder = EdgeCommonsBuilder.create("com.test.Component")
    args = ["-c", "FILE", "test-config.json", "--platform", "HOST", "--transport", "MQTT", "test-messaging-config.json"]
    
    result = builder.with_args(args)
    
    assert result == builder  # Fluent interface
    assert builder._args == args


def test_edgecommons_builder_with_app_options():
    """Test with_app_options method."""
    builder = EdgeCommonsBuilder.create("com.test.Component")
    options = Mock()
    
    result = builder.with_app_options(options)
    
    assert result == builder  # Fluent interface
    assert builder._app_options == options


def test_edgecommons_builder_receive_own_messages():
    """Test receive_own_messages method."""
    builder = EdgeCommonsBuilder.create("com.test.Component")
    
    result = builder.receive_own_messages(True)
    
    assert result == builder  # Fluent interface
    assert builder._receive_own_messages is True


def test_edgecommons_builder_build():
    """Test build method creates builder with correct parameters."""
    builder = EdgeCommonsBuilder.create("com.test.Component") \
        .with_args(["-c", "FILE", "test-config.json", "--platform", "HOST", "--transport", "MQTT", "test-messaging-config.json", "-t", "test-thing"]) \
        .receive_own_messages(False)
    
    # Test that builder has correct internal state before build
    assert builder._component_name == "com.test.Component"
    assert builder._args == ["-c", "FILE", "test-config.json", "--platform", "HOST", "--transport", "MQTT", "test-messaging-config.json", "-t", "test-thing"]
    assert builder._receive_own_messages is False
    assert builder._app_options is None


# MessageBuilder tests
def test_message_builder_create():
    """Test create method."""
    builder = MessageBuilder.create("TestMessage", "1.0")
    assert isinstance(builder, MessageBuilder)
    assert builder.name == "TestMessage"
    assert builder.version == "1.0"


def test_message_builder_with_payload():
    """Test with_payload method."""
    builder = MessageBuilder.create("TestMessage", "1.0")
    payload = {"data": "test"}
    
    result = builder.with_payload(payload)
    
    assert result == builder  # Fluent interface
    assert builder.payload == payload


def test_message_builder_with_config():
    """Test with_config method."""
    builder = MessageBuilder.create("TestMessage", "1.0")
    config = Mock()
    
    result = builder.with_config(config)
    
    assert result == builder  # Fluent interface
    assert builder.config_service == config


def test_message_builder_with_correlation_id():
    """Test with_correlation_id method."""
    builder = MessageBuilder.create("TestMessage", "1.0")
    
    result = builder.with_correlation_id("test-123")
    
    assert result == builder  # Fluent interface
    assert builder.correlation_id == "test-123"


def test_message_builder_build_without_config_is_bootstrap_shape():
    """Without a config service the builder produces a bootstrap-style envelope:
    header + body only (no identity, no tags) — UNS-CANONICAL-DESIGN §1.4."""
    builder = MessageBuilder.create("TestMessage", "1.0") \
        .with_payload({"data": "test"})

    m = builder.build()
    assert m.identity is None and m.tags is None
    assert m.get_body() == {"data": "test"}


def test_message_builder_build_with_config():
    """Test build method with config."""
    config = Mock()
    config.get_thing_name.return_value = "test-thing"
    config.get_tag_config.return_value = Mock()
    config.get_tag_config.return_value.to_dict.return_value = {}
    config.get_component_identity.return_value = None
    
    builder = MessageBuilder.create("TestMessage", "1.0") \
        .with_payload({"data": "test"}) \
        .with_config(config) \
        .with_correlation_id("test-123")
    
    result = builder.build()
    
    assert result is not None
    assert result.get_header().name == "TestMessage"
    assert result.get_header().version == "1.0"
    assert result.get_header().correlation_id == "test-123"
    assert result.get_body() == {"data": "test"}


# MetricBuilder tests
def test_metric_builder_create():
    """Test create method."""
    builder = MetricBuilder.create("test_metric")
    assert isinstance(builder, MetricBuilder)
    assert builder._name == "test_metric"


def test_metric_builder_with_namespace():
    """Test with_namespace method."""
    builder = MetricBuilder.create("test_metric")
    
    result = builder.with_namespace("TestApp/Metrics")
    
    assert result == builder  # Fluent interface
    assert builder._namespace == "TestApp/Metrics"


def test_metric_builder_add_measure():
    """Test add_measure method."""
    builder = MetricBuilder.create("test_metric")
    
    result = builder.add_measure("count", "Count", 60)
    
    assert result == builder  # Fluent interface
    assert len(builder._measures) == 1
    assert "count" in builder._measures
    measure = builder._measures["count"]
    assert measure.name == "count"
    assert measure.unit == "Count"
    assert measure.storage_resolution == 60


def test_metric_builder_add_dimension():
    """Test add_dimension method."""
    builder = MetricBuilder.create("test_metric")
    
    result = builder.add_dimension("instance", "main")
    
    assert result == builder  # Fluent interface
    assert builder._dimensions["instance"] == "main"


def test_metric_builder_build():
    """Test build method."""
    builder = MetricBuilder.create("test_metric") \
        .with_namespace("TestApp/Metrics") \
        .add_measure("count", "Count", 60) \
        .add_dimension("instance", "main")
    
    result = builder.build()
    
    assert result is not None
    assert result.name == "test_metric"
    assert result.namespace == "TestApp/Metrics"
    assert "instance" in result.dimensions
    assert result.dimensions["instance"] == "main"