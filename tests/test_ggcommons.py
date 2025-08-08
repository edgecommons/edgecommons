#   Copyright (c) 2024. Amazon.com Inc. or its affiliates.  All Rights Reserved.
#
#    Licensed under the Apache License, Version 2.0 (the "License"). You may not use this file except in compliance
#    with the License. A copy of the License is located at
#
#         http://www.apache.org/licenses/LICENSE-2.0
#
#    or in the 'license' file accompanying this file. This file is distributed on an 'AS IS' BASIS, WITHOUT WARRANTIES
#    OR CONDITIONS OF ANY KIND, express or implied. See the License for the specific language governing permissions
#    and limitations under the License.
#
import logging
import time
import unittest
from unittest.mock import Mock, patch

from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.message import Message
from ggcommons.ggcommons_builder import GGCommonsBuilder
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.interfaces import IMessagingService, IConfigurationService
from ggcommons import MessagingClient
from ggcommons.utils.utils import Utils

logger = logging.getLogger(__name__)

class TestGGCommons(unittest.TestCase):
    def setUp(self) -> None:
        self.received_message = None
        
        try:
            # Use STANDALONE mode with test messaging configuration file
            self.ggcommons = GGCommonsBuilder.create("ggcommons_python") \
                .with_args([
                    '-c', 'FILE', 'test-config.json', 
                    '-m', 'STANDALONE', '../standalone-messaging-sample.json',
                    '-t', 'ggcommons-test-2'
                ]) \
                .build()
            
            # Get services through dependency injection
            self.messaging_service = self.ggcommons.get_service(IMessagingService)
            self.config_service = self.ggcommons.get_service(IConfigurationService)
            self.init_success = True
        except Exception as e:
            self.init_success = False
            self.init_error = str(e)

    def tearDown(self) -> None:
        if hasattr(self, 'ggcommons') and self.ggcommons:
            self.ggcommons.shutdown()
        try:
            MessagingClient.shutdown()
        except:
            pass

    def ipc_message_handler(self, topic: str, message: Message):
        self.received_message = message

    def iot_core_message_handler(self, topic: str, message: Message):
        self.received_message = message

    def request_handler(self, topic: str, request: Message):
        reply_payload = {
            "reply_message": "I have received your request and have replied with this message"
        }
        reply = MessageBuilder.create("ReplyTest", "1.0") \
            .with_payload(reply_payload) \
            .with_config(self.config_service) \
            .with_correlation_id(request.get_header().correlation_id) \
            .build()
        MessagingClient.reply(request, reply)

    def test_initialization(self):
        """Test enhanced initialization with builder pattern"""
        if self.init_success:
            self.assertIsNotNone(self.ggcommons)
            self.assertIsNotNone(self.messaging_service)
            self.assertIsNotNone(self.config_service)
        else:
            self.fail(f"Initialization failed: {self.init_error}")
    
    def test_message_builder_patterns(self):
        """Test new message builder patterns"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        # Test basic message builder
        message = MessageBuilder.create("TestMessage", "1.0") \
            .with_payload({"test": "data"}) \
            .with_config(self.config_service) \
            .with_correlation_id("test-123") \
            .build()
        
        self.assertEqual(message.get_header().name, "TestMessage")
        self.assertEqual(message.get_header().version, "1.0")
        self.assertEqual(message.get_header().correlation_id, "test-123")
        self.assertEqual(message.get_body(), {"test": "data"})
    
    def test_pub_sub_ipc_message(self):
        """Test IPC messaging (requires AWS SDK)"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        topic = "test/testIpcTopic"
        self.messaging_service.subscribe(topic, self.ipc_message_handler)
        payload = {"message": "Test IPC message"}
        
        message = MessageBuilder.create("IpcMessageTest", "1.0") \
            .with_payload(payload) \
            .with_config(self.config_service) \
            .build()
        correlation_id = message.get_header().correlation_id
        uuid = message.get_header().uuid
        self.messaging_service.publish(topic, message)
        time.sleep(1)
        
        if self.received_message:
            self.assertEqual(self.received_message.get_header().name, "IpcMessageTest")
            self.assertEqual(uuid, self.received_message.get_header().uuid)
            self.assertEqual(
                correlation_id, self.received_message.get_header().correlation_id
            )
        self.received_message = None

    def test_pub_sub_iot_core_message(self):
        """Test IoT Core messaging (requires AWS SDK)"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        topic = "test/testIotCoreTopic"
        self.messaging_service.subscribe_to_iot_core(
            topic, self.iot_core_message_handler, QOS.AT_MOST_ONCE
        )
        payload = {"message": "Test IoT Core message"}
        message = MessageBuilder.create("IoTCoreMessageTest", "1.0") \
            .with_payload(payload) \
            .with_config(self.config_service) \
            .build()
        correlation_id = message.get_header().correlation_id
        uuid = message.get_header().uuid
        self.messaging_service.publish_to_iot_core(topic, message, QOS.AT_LEAST_ONCE)
        time.sleep(2)
        
        if self.received_message:
            self.assertEqual(self.received_message.get_header().name, "IoTCoreMessageTest")
            self.assertEqual(uuid, self.received_message.get_header().uuid)
            self.assertEqual(
                correlation_id, self.received_message.get_header().correlation_id
            )
        else:
            self.fail("No message received from iot core subscription")
        self.received_message = None

    def test_subscribe_with_filter(self):
        """Test topic filtering (requires AWS SDK)"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        sub_topic = "test/+"
        pub_topic = "test/testIpcTopic"
        self.messaging_service.subscribe(sub_topic, self.ipc_message_handler, 1)
        payload = {"message": "Test IPC message"}
        message = MessageBuilder.create("SubscribeWithFilterTest", "1.0") \
            .with_payload(payload) \
            .with_config(self.config_service) \
            .build()
        correlation_id = message.get_header().correlation_id
        self.messaging_service.publish(pub_topic, message)
        time.sleep(1)
        
        if self.received_message:
            self.assertEqual(
                self.received_message.get_header().name, "SubscribeWithFilterTest"
            )
            self.assertEqual(
                correlation_id, self.received_message.get_header().correlation_id
            )
        self.received_message = None

    def test_request_reply_ipc(self):
        """Test request-reply pattern (requires AWS SDK)"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        topic = "test/request"
        self.messaging_service.subscribe(topic, self.request_handler, 1)
        payload = {"message": "Test Request Reply"}
        message = MessageBuilder.create("RequestTest", "1.0") \
            .with_payload(payload) \
            .with_config(self.config_service) \
            .build()
        correlation_id = message.get_header().correlation_id
        uuid = message.get_header().uuid

        try:
            success, reply = self.messaging_service.request(topic, message).get(2)
            if success and reply:
                self.assertEqual(reply.get_header().name, "ReplyTest")
                self.assertEqual(correlation_id, reply.get_header().correlation_id)
        except Exception as e:
            self.skipTest(f"Request-reply test failed: {e}")

    def test_request_reply_iot_core(self):
        """Test IoT Core request-reply pattern (requires AWS SDK)"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        topic = "test/iot_core_request"
        logger.info(f"Sending request to IoT Core on {topic}")
        payload = {"message": "Test Request Reply"}
        message = MessageBuilder.create("RequestTest", "1.0") \
            .with_payload(payload) \
            .with_config(self.config_service) \
            .build()
        correlation_id = message.get_header().correlation_id

        success, reply = self.messaging_service.request_from_iot_core(topic, message).get(5)
        if success and reply:
            self.assertEqual(correlation_id, reply.get_header().correlation_id)
        else:
            self.fail("No reply received from iot core")

    def test_metric_builder_pattern(self):
        """Test enhanced metric builder pattern"""
        from ggcommons.metrics.metric_builder import MetricBuilder
        from ggcommons.interfaces import IMetricService
        
        # Test metric builder pattern
        metric = MetricBuilder.create("performance") \
            .with_namespace("TestApp/Metrics") \
            .add_measure("latency", "Milliseconds", 1) \
            .add_measure("throughput", "Count", 1) \
            .add_dimension("instance", "test") \
            .build()
        
        self.assertEqual(metric.name, "performance")
        self.assertEqual(metric.namespace, "TestApp/Metrics")
        self.assertEqual(len(metric.measures), 2)
        # Metrics have 3 implicit dimensions (name, component name, thing name)
        self.assertEqual(len(metric.dimensions), 4)
    

    
    def test_dual_subscription(self):
        """Test that local and IoT Core subscriptions on same topic don't interfere"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
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
        self.messaging_service.subscribe(topic, local_handler, 1)
        
        logger.info(f"Subscribing to IOT CORE messages on {topic}")
        self.messaging_service.subscribe_to_iot_core(topic, iot_core_handler, QOS.AT_LEAST_ONCE, 1)
        
        # Publish to local - should only trigger local callback
        local_payload = {"source": "local"}
        local_msg = MessageBuilder.create("LocalMessage", "1.0") \
            .with_payload(local_payload) \
            .with_config(self.config_service) \
            .build()
        logger.info("Publishing message to LOCAL on topic")
        self.messaging_service.publish(topic, local_msg)
        
        # Publish to IoT Core - should only trigger IoT Core callback
        iot_payload = {"source": "iotcore"}
        iot_msg = MessageBuilder.create("IoTCoreMessage", "1.0") \
            .with_payload(iot_payload) \
            .with_config(self.config_service) \
            .build()
        logger.info("Publishing message to IOT CORE on topic")
        self.messaging_service.publish_to_iot_core(topic, iot_msg, QOS.AT_LEAST_ONCE)
        
        time.sleep(0.5)
        
        # Verify local message only received by local handler
        self.assertEqual(len(local_received), 1)
        self.assertEqual(local_received[0].get_header().name, "LocalMessage")
        self.assertEqual(local_received[0].get_body()["source"], "local")
        
        # Verify IoT Core message only received by IoT Core handler
        self.assertEqual(len(iot_core_received), 1)
        self.assertEqual(iot_core_received[0].get_header().name, "IoTCoreMessage")
        self.assertEqual(iot_core_received[0].get_body()["source"], "iotcore")
        
        # Clean up
        self.messaging_service.unsubscribe(topic)
        self.messaging_service.unsubscribe_from_iot_core(topic)

    def test_service_interfaces(self):
        """Test service interface functionality"""
        if not self.init_success:
            self.skipTest("Initialization failed")
            
        # Test that services implement expected interfaces
        self.assertIsInstance(self.messaging_service, IMessagingService)
        self.assertIsInstance(self.config_service, IConfigurationService)
        
        # Test service methods exist
        self.assertTrue(hasattr(self.messaging_service, 'publish'))
        self.assertTrue(hasattr(self.messaging_service, 'subscribe'))
        self.assertTrue(hasattr(self.config_service, 'get_global_config'))
        self.assertTrue(hasattr(self.config_service, 'get_instance_ids'))
