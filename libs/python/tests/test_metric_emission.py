"""
Integration tests for metric emission functionality.
"""
import math

import pytest
import logging
import time
import json
import tempfile
import os

from ggcommons.messaging.message import Message
from ggcommons.ggcommons_builder import GGCommonsBuilder
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter

logger = logging.getLogger(__name__)

# Get the directory where this test file is located
TEST_DIR = os.path.dirname(os.path.abspath(__file__))
MESSAGING_CONFIG_PATH = os.path.join(TEST_DIR, 'test-messaging-config.json')


@pytest.fixture
def metric_messaging_config():
    """Create temporary metric messaging configuration."""
    config = {
        "metricEmission": {
            "target": "messaging",
            "namespace": "TestApp/Metrics",
            "targetConfig": {
                "topic": "metrics/{ComponentName}/{ThingName}",
                "destination": "local"
            }
        },
        "component": {
            "global": {"test": "value"},
            "instances": [{"id": "main"}]
        }
    }
    
    with tempfile.NamedTemporaryFile(mode='w', suffix='.json', delete=False) as f:
        json.dump(config, f)
        temp_path = f.name
    
    yield temp_path
    
    if os.path.exists(temp_path):
        os.unlink(temp_path)


@pytest.fixture
def ggcommons_metric_messaging(metric_messaging_config):
    """Create GGCommons instance with metric messaging configuration."""
    try:
        ggcommons = GGCommonsBuilder.create("metric_test") \
            .with_args([
                '-c', 'FILE', metric_messaging_config,
                '-m', 'STANDALONE', MESSAGING_CONFIG_PATH,
                '-t', 'metric-test-thing'
            ]) \
            .build()
        
        yield ggcommons
        
    except Exception as e:
        pytest.skip(f"Failed to initialize GGCommons for metric messaging test: {e}")
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.mark.integration
def test_metric_emission_emf_format(ggcommons_metric_messaging):
    """Test metric emission produces EMF format messages."""
    messaging_service = ggcommons_metric_messaging.get_messaging()
    metric_service = ggcommons_metric_messaging.get_metrics()
    received_messages = []
    
    def metric_handler(topic: str, message: Message):
        logger.info(f"Received metric message on topic: {topic}")
        received_messages.append((topic, message))
    
    # Subscribe to metrics topic
    metrics_topic = "metrics/metric_test/metric-test-thing"
    messaging_service.subscribe(metrics_topic, metric_handler)
    
    # Define and emit a custom metric
    metric = MetricBuilder.create("test_metric") \
        .with_namespace("TestApp/Metrics") \
        .add_measure("count", "Count", 1) \
        .add_measure("latency", "Milliseconds", 1) \
        .add_dimension("instance", "main") \
        .build()
    
    metric_service.define_metric(metric)
    
    # Emit metric values
    values = {
        "count": 42.0,
        "latency": 150.5
    }
    metric_service.emit_metric_now("test_metric", values)
    
    # Wait for metric message
    time.sleep(2)
    
    # Verify we received metric message
    assert len(received_messages) >= 1, "Should have received at least one metric message"
    
    topic, message = received_messages[0]
    assert topic == metrics_topic
    
    # Verify EMF format structure
    body = message.get_body()
    
    # EMF required fields
    assert "_aws" in body, "EMF message must contain _aws field"
    assert "CloudWatchMetrics" in body["_aws"], "EMF message must contain CloudWatchMetrics"
    assert "Timestamp" in body["_aws"], "EMF message must contain Timestamp"
    
    # Verify CloudWatchMetrics structure
    cw_metrics = body["_aws"]["CloudWatchMetrics"]
    assert isinstance(cw_metrics, list), "CloudWatchMetrics must be a list"
    assert len(cw_metrics) >= 1, "CloudWatchMetrics must contain at least one entry"
    
    metric_entry = cw_metrics[0]
    assert "Namespace" in metric_entry, "Metric entry must contain Namespace"
    assert "Dimensions" in metric_entry, "Metric entry must contain Dimensions"
    assert "Metrics" in metric_entry, "Metric entry must contain Metrics"
    
    # Verify namespace
    assert metric_entry["Namespace"] == "TestApp/Metrics"
    
    # Verify dimensions
    dimensions = metric_entry["Dimensions"]
    assert isinstance(dimensions, list), "Dimensions must be a list"
    
    # Verify metrics
    metrics = metric_entry["Metrics"]
    assert isinstance(metrics, list), "Metrics must be a list"
    assert len(metrics) == 2, "Should have 2 metrics (count and latency)"
    
    metric_names = [m["Name"] for m in metrics]
    assert "count" in metric_names, "Should contain count metric"
    assert "latency" in metric_names, "Should contain latency metric"
    
    # Verify measure values are present as top-level properties
    assert "count" in body, "Count value must be present as top-level property"
    assert "latency" in body, "Latency value must be present as top-level property"
    assert body["count"] == 42.0, "Count value should match emitted value"
    assert body["latency"] == 150.5, "Latency value should match emitted value"
    
    # Verify dimension values are present as top-level properties
    assert "instance" in body, "Dimension values must be present as top-level properties"
    assert body["instance"] == "main", "Instance dimension should match defined value"


@pytest.mark.integration
def test_metric_emission_multiple_metrics(ggcommons_metric_messaging):
    """Test emission of multiple different metrics."""
    messaging_service = ggcommons_metric_messaging.get_messaging()
    metric_service = ggcommons_metric_messaging.get_metrics()
    received_messages = []
    
    def metric_handler(topic: str, message: Message):
        received_messages.append(message)
    
    # Subscribe to metrics topic
    metrics_topic = "metrics/metric_test/metric-test-thing"
    messaging_service.subscribe(metrics_topic, metric_handler)
    
    # Define multiple metrics
    performance_metric = MetricBuilder.create("performance") \
        .with_namespace("TestApp/Performance") \
        .add_measure("throughput", "Count", 1) \
        .add_measure("errors", "Count", 1) \
        .build()
    
    resource_metric = MetricBuilder.create("resources") \
        .with_namespace("TestApp/Resources") \
        .add_measure("cpu_usage", "Percent", 1) \
        .add_measure("memory_usage", "Bytes", 1) \
        .build()
    
    metric_service.define_metric(performance_metric)
    metric_service.define_metric(resource_metric)
    
    # Emit different metrics
    metric_service.emit_metric("performance", {"throughput": 100.0, "errors": 2.0})
    metric_service.emit_metric("resources", {"cpu_usage": 75.5, "memory_usage": 1024000.0})
    
    # Wait for metric messages
    time.sleep(2)
    
    # Verify we received multiple metric messages
    assert len(received_messages) >= 2, "Should have received at least two metric messages"
    
    # Verify each message has proper EMF format
    for message in received_messages:
        body = message.get_body()
        assert "_aws" in body, "Each metric message must be in EMF format"
        assert "CloudWatchMetrics" in body["_aws"]


@pytest.mark.integration
def test_metric_emission_timestamp_format(ggcommons_metric_messaging):
    """Test that metric emission includes proper timestamp in EMF format."""
    messaging_service = ggcommons_metric_messaging.get_messaging()
    metric_service = ggcommons_metric_messaging.get_metrics()
    received_messages = []
    
    def metric_handler(topic: str, message: Message):
        received_messages.append(message)
    
    # Subscribe to metrics topic
    metrics_topic = "metrics/metric_test/metric-test-thing"
    messaging_service.subscribe(metrics_topic, metric_handler)
    
    # Define and emit metric
    metric = MetricBuilder.create("timestamp_test") \
        .add_measure("value", "Count", 1) \
        .build()
    
    metric_service.define_metric(metric)
    
    start_time = math.floor(time.time() * 1000)  # Convert to milliseconds
    metric_service.emit_metric("timestamp_test", {"value": 1.0})
    end_time = time.time() * 1000
    
    # Wait for metric message
    time.sleep(2)
    
    assert len(received_messages) >= 1, "Should have received metric message"
    
    body = received_messages[0].get_body()
    timestamp = body["_aws"]["Timestamp"]
    
    # Verify timestamp is in milliseconds and within expected range
    assert isinstance(timestamp, (int, float)), "Timestamp should be numeric"
    assert start_time <= timestamp <= end_time, "Timestamp should be within emission time range"

def test_cloudwatch_target_skips_undefined_measure():
    """Regression: emitting a measure name the metric never defined must NOT crash.

    Previously CloudWatch._prepare_metric_data did metric.get_measure(name).get_unit(), raising
    AttributeError on None when the emit named an undefined measure (e.g. a component emitting
    'replyLatency' against a metric that only defined 'latency'). That propagated out of
    emit_metric and crashed the whole component. The target must skip the unknown data point.
    """
    import logging
    from unittest.mock import MagicMock

    from ggcommons.metrics.targets.cloudwatch import CloudWatch

    metric = (
        MetricBuilder.create("performance")
        .with_thing_name("thing")
        .with_component_name("comp")
        .add_measure("latency", "Milliseconds", 1)
        .build()
    )

    # Exercise the pure data-prep path without the boto3 client created in __init__.
    target = CloudWatch.__new__(CloudWatch)
    target.logger = logging.getLogger("test-cw")
    target.metric_config = MagicMock()
    target.metric_config.get_large_fleet_workaround.return_value = False

    data = target._prepare_metric_data(metric, {"latency": 12.5, "replyLatency": 99.0})

    names = [d["MetricName"] for d in data]
    assert names == ["latency"], "unknown measure 'replyLatency' must be skipped, not crash"
    assert data[0]["Unit"] == "Milliseconds"
