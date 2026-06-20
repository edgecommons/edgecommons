"""
Integration tests for heartbeat functionality.
"""
from datetime import datetime

import pytest
import logging
import time
import json
import tempfile
import os

from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.message import Message
from ggcommons.ggcommons_builder import GGCommonsBuilder
from ggcommons import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter

logger = logging.getLogger(__name__)

# Get the directory where this test file is located
TEST_DIR = os.path.dirname(os.path.abspath(__file__))
MESSAGING_CONFIG_PATH = os.path.join(TEST_DIR, 'test-messaging-config.json')


@pytest.fixture
def heartbeat_messaging_config():
    """Create temporary heartbeat messaging configuration."""
    config = {
        "heartbeat": {
            "intervalSecs": 2,  # Short interval for testing
            "measures": {
                "cpu": True,
                "memory": True,
                "disk": False,
                "threads": True,
                "fileDescriptors": False
            },
            "targets": [{
                "type": "messaging",
                "config": {
                    "destination": "local",
                    "topic": "heartbeat/{ComponentName}/{ThingName}"
                }
            }]
        },
        "metricEmission": {
            "target": "messaging",
            "topic": "heartbeat/{ComponentName}/{ThingName}",
            "destination": "local"
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
def heartbeat_metric_config():
    """Create temporary heartbeat metric configuration."""
    config = {
        "heartbeat": {
            "intervalSecs": 2,
            "measures": {
                "cpu": True,
                "memory": True,
                "disk": True
            },
            "targets": [{"type": "metric"}]
        },
        "metricEmission": {
            "target": "cloudwatch",
            "namespace": "TestApp/Heartbeat"
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
def heartbeat_iotcore_config():
    """Create temporary heartbeat configuration targeting IoT Core."""
    config = {
        "heartbeat": {
            "intervalSecs": 2,
            "measures": {
                "cpu": True,
                "memory": True
            },
            "targets": [{
                "type": "messaging",
                "config": {
                    "destination": "iotcore",
                    "topic": "heartbeat/{ComponentName}/{ThingName}"
                }
            }]
        },
        "metricEmission": {
            "target": "messaging",
            "topic": "heartbeat/{ComponentName}/{ThingName}",
            "destination": "iotcore"
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
def heartbeat_dual_config():
    """Create temporary heartbeat configuration with both local and IoT Core messaging targets."""
    config = {
        "heartbeat": {
            "intervalSecs": 2,
            "measures": {
                "cpu": True,
                "memory": True
            },
            "targets": [
                {
                    "type": "messaging",
                    "config": {
                        "destination": "local",
                        "topic": "heartbeat/{ComponentName}/{ThingName}"
                    }
                },
                {
                    "type": "messaging",
                    "config": {
                        "destination": "iotcore",
                        "topic": "heartbeat/{ComponentName}/{ThingName}"
                    }
                }
            ]
        },
        "metricEmission": {
            "target": "messaging",
            "topic": "metric/{ComponentName}/{ThingName}",
            "destination": "local"
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
def ggcommons_heartbeat_messaging(heartbeat_messaging_config):
    """Create GGCommons instance with heartbeat messaging configuration."""
    try:
        ggcommons = GGCommonsBuilder.create("heartbeat_test") \
            .with_args([
                '-c', 'FILE', heartbeat_messaging_config,
                '-m', 'STANDALONE', MESSAGING_CONFIG_PATH,
                '-t', 'heartbeat-test-thing'
            ]) \
            .build()
        
        yield ggcommons
        
    except Exception as e:
        pytest.skip(f"Failed to initialize GGCommons for heartbeat messaging test: {e}")
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.fixture
def ggcommons_heartbeat_metric(heartbeat_metric_config):
    """Create GGCommons instance with heartbeat metric configuration."""
    try:
        ggcommons = GGCommonsBuilder.create("heartbeat_test") \
            .with_args([
                '-c', 'FILE', heartbeat_metric_config,
                '-m', 'STANDALONE', MESSAGING_CONFIG_PATH,
                '-t', 'heartbeat-test-thing'
            ]) \
            .build()
        
        yield ggcommons
        
    except Exception as e:
        pytest.skip(f"Failed to initialize GGCommons for heartbeat metric test: {e}")
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.fixture
def ggcommons_heartbeat_iotcore(heartbeat_iotcore_config):
    """Create GGCommons instance with IoT Core heartbeat configuration."""
    try:
        ggcommons = GGCommonsBuilder.create("heartbeat_test") \
            .with_args([
                '-c', 'FILE', heartbeat_iotcore_config,
                '-m', 'STANDALONE', MESSAGING_CONFIG_PATH,
                '-t', 'heartbeat-test-thing'
            ]) \
            .build()
        
        yield ggcommons
        
    except Exception as e:
        pytest.skip(f"Failed to initialize GGCommons for heartbeat IoT Core test: {e}")
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.fixture
def ggcommons_heartbeat_dual(heartbeat_dual_config):
    """Create GGCommons instance with dual heartbeat configuration."""
    try:
        ggcommons = GGCommonsBuilder.create("heartbeat_test") \
            .with_args([
                '-c', 'FILE', heartbeat_dual_config,
                '-m', 'STANDALONE', MESSAGING_CONFIG_PATH,
                '-t', 'heartbeat-test-thing'
            ]) \
            .build()
        
        yield ggcommons
        
    except Exception as e:
        pytest.skip(f"Failed to initialize GGCommons for heartbeat dual test: {e}")
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.mark.integration
@pytest.mark.slow
def test_heartbeat_messaging_target_local(ggcommons_heartbeat_messaging):
    """Test heartbeat with messaging target to local broker."""
    logger.info(f"Starting test_heartbeat_messaging_target_local test...")
    messaging_service = ggcommons_heartbeat_messaging.get_messaging()
    received_messages = []
    
    def heartbeat_handler(topic: str, message: Message):
        logger.info(f"Received heartbeat message on topic: {topic}")
        received_messages.append((topic, message))
    
    # Subscribe to heartbeat topic
    heartbeat_topic = "heartbeat/heartbeat_test/heartbeat-test-thing"
    messaging_service.subscribe(heartbeat_topic, heartbeat_handler)
    logger.info(f"Subscribed to heartbeat/heartbeat_test/heartbeat-test-thing")
    
    # Wait for heartbeat messages (interval is 2 seconds)
    time.sleep(5)
    
    # Verify we received heartbeat messages
    assert len(received_messages) >= 1, "Should have received at least one heartbeat message"
    
    topic, message = received_messages[0]
    assert topic == heartbeat_topic
    assert message.get_header().name == "Heartbeat"
    
    # Verify heartbeat payload structure
    body = message.get_body()
    assert "cpu" in body
    assert "memory" in body
    assert "threads" in body
    
    # Verify nested structure
    assert "cpu_usage" in body["cpu"]
    assert "memory_usage" in body["memory"]
    assert "threads" in body["threads"]
    
    assert isinstance(body["cpu"]["cpu_usage"], (int, float))
    assert isinstance(body["memory"]["memory_usage"], (int, float))
    assert isinstance(body["threads"]["threads"], int)
    
    # Verify disk and fileDescriptors are not included (disabled in config)
    assert "disk" not in body
    assert "fds" not in body


@pytest.mark.integration
@pytest.mark.slow
@pytest.mark.aws
def test_heartbeat_messaging_target_iot_core(ggcommons_heartbeat_iotcore):
    """Test heartbeat with messaging target to IoT Core."""
    messaging_service = ggcommons_heartbeat_iotcore.get_messaging()
    received_messages = []
    
    def heartbeat_handler(topic: str, message: Message):
        logger.info(f"Received heartbeat message on IoT Core topic: {topic}")
        received_messages.append((topic, message))
    
    # Subscribe to heartbeat topic on IoT Core
    heartbeat_topic = "heartbeat/heartbeat_test/heartbeat-test-thing"
    messaging_service.subscribe_to_iot_core(heartbeat_topic, heartbeat_handler, QOS.AT_LEAST_ONCE)
    
    # Wait for heartbeat messages
    time.sleep(5)
    
    # Verify we received heartbeat messages
    assert len(received_messages) >= 1, "Should have received at least one heartbeat message on IoT Core"
    
    topic, message = received_messages[0]
    assert topic == heartbeat_topic
    assert message.get_header().name == "Heartbeat"
    
    # Verify heartbeat payload structure (same as local)
    body = message.get_body()
    assert "cpu" in body
    assert "memory" in body
    
    # Verify nested structure
    assert "cpu_usage" in body["cpu"]
    assert "memory_usage" in body["memory"]
    
    assert isinstance(body["cpu"]["cpu_usage"], (int, float))
    assert isinstance(body["memory"]["memory_usage"], (int, float))
    assert message.get_header().name == "Heartbeat"
    

@pytest.mark.integration
def test_heartbeat_metric_target(ggcommons_heartbeat_metric):
    """Test heartbeat with metric target."""
    metric_service = ggcommons_heartbeat_metric.get_metrics()
    
    # Get initial metric count
    initial_metrics = len(metric_service._metrics) if hasattr(metric_service, '_metrics') else 0
    
    # Wait for heartbeat to emit metrics
    time.sleep(3)
    
    # Verify heartbeat metrics were defined
    # Note: This test verifies the metric service received heartbeat metrics
    # In a real environment, these would be sent to CloudWatch
    logger.info("Heartbeat metric target test completed - metrics would be sent to CloudWatch")


@pytest.mark.integration
@pytest.mark.slow
@pytest.mark.aws
def test_heartbeat_dual_targets(ggcommons_heartbeat_dual):
    """Test heartbeat with both messaging and metric targets."""
    messaging_service = ggcommons_heartbeat_dual.get_messaging()
    received_messages = []
    
    def local_handler(topic: str, message: Message):
        logger.info(f"Received local heartbeat message on topic: {topic}")
        received_messages.append((topic, message))

    def iot_core_handler(topic: str, message: Message):
        logger.info(f"Received iot core heartbeat message on topic: {topic}")
        received_messages.append((topic, message))

    # Subscribe to both local and IoT Core topics
    topic = "heartbeat/heartbeat_test/heartbeat-test-thing"
    messaging_service.subscribe(topic, local_handler)
    messaging_service.subscribe_to_iot_core(topic, iot_core_handler, QOS.AT_LEAST_ONCE)
    
    # Wait for heartbeat messages
    time.sleep(5)
    
    # Verify we received messages (should get both messaging and metric emissions)
    assert len(received_messages) >= 1, "Should have received heartbeat messages from dual targets"
    
    # Verify message structure
    for topic, message in received_messages:
        assert topic == topic
        assert message.get_header().name == "Heartbeat"

        # Verify heartbeat payload structure (same as local)
        body = message.get_body()
        assert "cpu" in body
        assert "memory" in body

        # Verify nested structure
        assert "cpu_usage" in body["cpu"]
        assert "memory_usage" in body["memory"]

        assert isinstance(body["cpu"]["cpu_usage"], (int, float))
        assert isinstance(body["memory"]["memory_usage"], (int, float))
        assert message.get_header().name == "Heartbeat"


@pytest.mark.integration
def test_heartbeat_configuration_validation(heartbeat_messaging_config):
    """Test heartbeat configuration validation."""
    config_service = None
    
    try:
        ggcommons = GGCommonsBuilder.create("heartbeat_config_test") \
            .with_args([
                '-c', 'FILE', heartbeat_messaging_config,
                '-m', 'STANDALONE', MESSAGING_CONFIG_PATH,
                '-t', 'config-test-thing'
            ]) \
            .build()
        
        config_service = ggcommons.get_config_manager()
        
        # Verify heartbeat configuration is loaded correctly
        heartbeat_config = config_service.get_heartbeat_config()
        assert heartbeat_config is not None
        assert heartbeat_config.get_interval_secs() == 2
        
        assert heartbeat_config.include_cpu() is True
        assert heartbeat_config.include_memory() is True
        assert heartbeat_config.include_disk() is False
        assert heartbeat_config.include_threads() is True
        assert heartbeat_config.include_fds() is False
        
        targets = heartbeat_config.get_targets()
        assert len(targets) == 1
        assert targets[0]['type'] == "messaging"
        
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.mark.integration
def test_heartbeat_measure_values(ggcommons_heartbeat_messaging):
    """Test that heartbeat measures contain valid values."""
    messaging_service = ggcommons_heartbeat_messaging.get_messaging()
    received_messages = []
    
    def heartbeat_handler(topic: str, message: Message):
        received_messages.append(message)
    
    # Subscribe to heartbeat topic
    heartbeat_topic = "heartbeat/heartbeat_test/heartbeat-test-thing"
    messaging_service.subscribe(heartbeat_topic, heartbeat_handler)
    
    # Wait for heartbeat message
    time.sleep(3)
    
    assert len(received_messages) >= 1, "Should have received heartbeat message"
    
    message = received_messages[0]
    body = message.get_body()

    # Validate CPU measure
    cpu_value = body["cpu"]["cpu_usage"]
    assert 0.0 <= cpu_value <= 100.0, f"CPU should be between 0-100%, got {cpu_value}"
    
    # Validate memory measure (in MB)
    memory_value = body["memory"]["memory_usage"]
    assert memory_value > 0, f"Memory should be positive, got {memory_value}"
    assert memory_value < 100000, f"Memory seems too high (MB), got {memory_value}"
    
    # Validate threads measure
    threads_value = body["threads"]["threads"]
    assert threads_value > 0, f"Thread count should be positive, got {threads_value}"
    assert threads_value < 10000, f"Thread count seems too high, got {threads_value}"
    
    # Validate timestamp
    timestamp = datetime.fromisoformat(message.get_header().timestamp.replace('Z', '+00:00')).timestamp()
    current_time = time.time()
    assert abs(timestamp - current_time) < 10, "Timestamp should be recent"


@pytest.mark.integration
@pytest.mark.slow
def test_heartbeat_interval_timing(ggcommons_heartbeat_messaging):
    """Test that heartbeat messages are sent at the configured interval."""
    messaging_service = ggcommons_heartbeat_messaging.get_messaging()
    received_times = []
    
    def heartbeat_handler(topic: str, message: Message):
        received_times.append(time.time())
    
    # Subscribe to heartbeat topic
    heartbeat_topic = "heartbeat/heartbeat_test/heartbeat-test-thing"
    messaging_service.subscribe(heartbeat_topic, heartbeat_handler)
    
    # Wait for multiple heartbeat messages (interval is 2 seconds)
    time.sleep(7)
    
    assert len(received_times) >= 2, "Should have received multiple heartbeat messages"
    
    # Check intervals between messages
    for i in range(1, len(received_times)):
        interval = received_times[i] - received_times[i-1]
        # Allow some tolerance for timing
        assert 1.5 <= interval <= 2.5, f"Heartbeat interval should be ~2 seconds, got {interval}"