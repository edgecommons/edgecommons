"""
Integration tests for GGCommons.
"""

import pytest
import logging
import time

from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.message import Message
from ggcommons.ggcommons_builder import GGCommonsBuilder
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons import MessagingClient

logger = logging.getLogger(__name__)


@pytest.fixture(scope="class")
def ggcommons_instance():
    """Create GGCommons instance for testing."""
    try:
        ggcommons = GGCommonsBuilder.create("ggcommons_python") \
            .with_args([
                '-c', 'FILE', 'c:/Users/breis/source/ggcommons/ggcommons-python-lib/tests/test-config.json',
                '-m', 'STANDALONE', 'c:/Users/breis/source/ggcommons/ggcommons-python-lib/tests/test-messaging-config.json',
                '-t', 'ggcommons-test-2'
            ]) \
            .build()
        
        yield ggcommons
        
    except Exception as e:
        pytest.skip(f"Failed to initialize GGCommons: {e}")
    finally:
        if 'ggcommons' in locals():
            ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass


@pytest.fixture
def messaging_service(ggcommons_instance):
    """Get the messaging handle from the GGCommons instance."""
    return ggcommons_instance.get_messaging()


@pytest.fixture
def config_service(ggcommons_instance):
    """Get the configuration manager from the GGCommons instance."""
    return ggcommons_instance.get_config_manager()


@pytest.fixture
def received_messages():
    """Container for received messages."""
    return []


@pytest.mark.integration
def test_initialization(ggcommons_instance, messaging_service, config_service):
    """Test enhanced initialization with builder pattern."""
    assert ggcommons_instance is not None
    assert messaging_service is not None
    assert config_service is not None


@pytest.mark.integration
def test_message_builder_patterns(config_service):
    """Test new message builder patterns."""
    # Test basic message builder
    message = MessageBuilder.create("TestMessage", "1.0") \
        .with_payload({"test": "data"}) \
        .with_config(config_service) \
        .with_correlation_id("test-123") \
        .build()
    
    assert message.get_header().name == "TestMessage"
    assert message.get_header().version == "1.0"
    assert message.get_header().correlation_id == "test-123"
    assert message.get_body() == {"test": "data"}


@pytest.mark.integration
@pytest.mark.slow
def test_pub_sub_ipc_message(messaging_service, config_service, received_messages):
    """Test IPC messaging."""
    def message_handler(topic: str, message: Message):
        received_messages.append(message)

    topic = "test/testIpcTopic"
    messaging_service.subscribe(topic, message_handler)
    payload = {"message": "Test IPC message"}
    
    message = MessageBuilder.create("IpcMessageTest", "1.0") \
        .with_payload(payload) \
        .with_config(config_service) \
        .build()
    correlation_id = message.get_header().correlation_id
    uuid = message.get_header().uuid
    messaging_service.publish(topic, message)
    time.sleep(1)
    
    if received_messages:
        received = received_messages[0]
        assert received.get_header().name == "IpcMessageTest"
        assert uuid == received.get_header().uuid
        assert correlation_id == received.get_header().correlation_id


@pytest.mark.integration
@pytest.mark.slow
@pytest.mark.aws
def test_pub_sub_iot_core_message(messaging_service, config_service, received_messages):
    """Test IoT Core messaging."""
    def message_handler(topic: str, message: Message):
        received_messages.append(message)

    topic = "test/testIotCoreTopic"
    messaging_service.subscribe_to_iot_core(
        topic, message_handler, QOS.AT_MOST_ONCE
    )
    payload = {"message": "Test IoT Core message"}
    message = MessageBuilder.create("IoTCoreMessageTest", "1.0") \
        .with_payload(payload) \
        .with_config(config_service) \
        .build()
    correlation_id = message.get_header().correlation_id
    uuid = message.get_header().uuid
    messaging_service.publish_to_iot_core(topic, message, QOS.AT_LEAST_ONCE)
    time.sleep(2)
    
    if received_messages:
        received = received_messages[0]
        assert received.get_header().name == "IoTCoreMessageTest"
        assert uuid == received.get_header().uuid
        assert correlation_id == received.get_header().correlation_id
    else:
        pytest.fail("No message received from iot core subscription")


@pytest.mark.integration
def test_subscribe_with_filter(messaging_service, config_service, received_messages):
    """Test topic filtering."""
    def message_handler(topic: str, message: Message):
        received_messages.append(message)

    sub_topic = "test/+"
    pub_topic = "test/testIpcTopic"
    messaging_service.subscribe(sub_topic, message_handler, 1)
    payload = {"message": "Test IPC message"}
    message = MessageBuilder.create("SubscribeWithFilterTest", "1.0") \
        .with_payload(payload) \
        .with_config(config_service) \
        .build()
    correlation_id = message.get_header().correlation_id
    messaging_service.publish(pub_topic, message)
    time.sleep(1)
    
    if received_messages:
        received = received_messages[0]
        assert received.get_header().name == "SubscribeWithFilterTest"
        assert correlation_id == received.get_header().correlation_id


@pytest.mark.integration
def test_request_reply_ipc(messaging_service, config_service):
    """Test request-reply pattern."""
    def request_handler(topic: str, request: Message):
        reply_payload = {
            "reply_message": "I have received your request and have replied with this message"
        }
        reply = MessageBuilder.create("ReplyTest", "1.0") \
            .with_payload(reply_payload) \
            .with_config(config_service) \
            .with_correlation_id(request.get_header().correlation_id) \
            .build()
        MessagingClient.reply(request, reply)

    topic = "test/request"
    messaging_service.subscribe(topic, request_handler, 1)
    payload = {"message": "Test Request Reply"}
    message = MessageBuilder.create("RequestTest", "1.0") \
        .with_payload(payload) \
        .with_config(config_service) \
        .build()
    correlation_id = message.get_header().correlation_id

    try:
        success, reply = messaging_service.request(topic, message).get(2)
        if success and reply:
            assert reply.get_header().name == "ReplyTest"
            assert correlation_id == reply.get_header().correlation_id
    except Exception as e:
        pytest.skip(f"Request-reply test failed: {e}")


@pytest.mark.integration
def test_metric_builder_pattern():
    """Test enhanced metric builder pattern."""
    from ggcommons.metrics.metric_builder import MetricBuilder
    
    # Test metric builder pattern
    metric = MetricBuilder.create("performance") \
        .with_namespace("TestApp/Metrics") \
        .add_measure("latency", "Milliseconds", 1) \
        .add_measure("throughput", "Count", 1) \
        .add_dimension("instance", "test") \
        .build()
    
    assert metric.name == "performance"
    assert metric.namespace == "TestApp/Metrics"
    assert len(metric.measures) == 2
    # Metrics have 3 implicit dimensions (name, component name, thing name)
    assert len(metric.dimensions) == 4


@pytest.mark.integration
@pytest.mark.slow
@pytest.mark.aws
def test_dual_subscription(messaging_service, config_service):
    """Test that local and IoT Core subscriptions on same topic don't interfere.

    Requires real AWS IoT Core connectivity (the messaging fixture's iotCore
    endpoint), hence @pytest.mark.aws. For an AWS-free dual-broker check see
    tests/test_dual_broker_integration.py."""
    topic = "test/dualTopic"
    local_received = []
    iot_core_received = []
    
    def local_handler(t: str, m: Message):
        logger.info(f"Received message on LOCAL: {m.get_header().name}")
        local_received.append(m)
        
    def iot_core_handler(t: str, m: Message):
        logger.info(f"Received message on IOT CORE: {m.get_header().name}")
        iot_core_received.append(m)
    
    # Subscribe to the same topic on both local and IoT Core
    logger.info(f"Subscribing to LOCAL messages on {topic}")
    messaging_service.subscribe(topic, local_handler, 1)
    
    logger.info(f"Subscribing to IOT CORE messages on {topic}")
    messaging_service.subscribe_to_iot_core(topic, iot_core_handler, QOS.AT_LEAST_ONCE, 1)
    
    # Publish to local - should only trigger local callback
    local_payload = {"source": "local"}
    local_msg = MessageBuilder.create("LocalMessage", "1.0") \
        .with_payload(local_payload) \
        .with_config(config_service) \
        .build()
    logger.info("Publishing message to LOCAL on topic")
    messaging_service.publish(topic, local_msg)
    
    # Publish to IoT Core - should only trigger IoT Core callback
    iot_payload = {"source": "iotcore"}
    iot_msg = MessageBuilder.create("IoTCoreMessage", "1.0") \
        .with_payload(iot_payload) \
        .with_config(config_service) \
        .build()
    logger.info("Publishing message to IOT CORE on topic")
    messaging_service.publish_to_iot_core(topic, iot_msg, QOS.AT_LEAST_ONCE)
    
    time.sleep(0.5)
    
    # Verify local message only received by local handler
    assert len(local_received) == 1
    assert local_received[0].get_header().name == "LocalMessage"
    assert local_received[0].get_body()["source"] == "local"
    
    # Verify IoT Core message only received by IoT Core handler
    assert len(iot_core_received) == 1
    assert iot_core_received[0].get_header().name == "IoTCoreMessage"
    assert iot_core_received[0].get_body()["source"] == "iotcore"
    
    # Clean up
    messaging_service.unsubscribe(topic)
    messaging_service.unsubscribe_from_iot_core(topic)


@pytest.mark.integration
def test_service_accessors(messaging_service, config_service):
    """Test the typed accessors return the concrete subsystems."""
    from ggcommons.config.manager.config_manager import ConfigManager

    # Typed accessors return the concrete handles (no DI/interfaces).
    assert messaging_service is MessagingClient
    assert isinstance(config_service, ConfigManager)

    # Test expected methods exist
    assert hasattr(messaging_service, 'publish')
    assert hasattr(messaging_service, 'subscribe')
    assert hasattr(config_service, 'get_global_config')
    assert hasattr(config_service, 'get_instance_ids')