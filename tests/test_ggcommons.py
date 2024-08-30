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
import time
from unittest import TestCase
from awsiot.greengrasscoreipc.model import QOS
from ggcommons import MessagingClient
from ggcommons.messaging.message import Message, MessageBuilder


class TestGGCommons(TestCase):

    def setUp(self) -> None:
        import argparse
        import sys
        import ggcommons

        self.received_message = None
        sys.argv = [
            "ggcommons_python",
            "--config", "FILE", "../config_3.json",
            "--messaging", "MQTT", "localhost", "1883",
            # "--messaging", "MQTT", "a3bgkcole5zuv-ats.iot.us-east-1.amazonaws.com", "443", "../creds",
            "--thing", "ggcommons-test-2"
        ]
        self.args, self.config_mgr, self.heartbeat_mgr = ggcommons.init("ggcommons_python", argparse.ArgumentParser())

    def tearDown(self) -> None:
        self.heartbeat_mgr.stop()
        MessagingClient.shutdown()

    def ipc_message_handler(self, topic: str, message: Message):
        self.received_message = message

    def iot_core_message_handler(self, topic: str, message: Message):
        self.received_message = message

    def request_handler(self, topic: str, request: Message):
        reply_payload = {"reply_message": "I have received your request and have replied with this message"}
        reply = MessageBuilder.build_response("ReplyTest", "1.0", reply_payload, self.config_mgr, request)
        MessagingClient.reply(request, reply)

    def iot_core_request_handler(self, topic: str, request: Message):
        reply_payload = {"reply_message": "(IoT Core) I have received your request and have replied with this message"}
        reply = MessageBuilder.build_response("ReplyTest", "1.0", reply_payload, self.config_mgr, request)
        MessagingClient.reply(request, reply)

    def test_pub_sub_ipc_message(self):
        topic = "test/testIpcTopic"
        MessagingClient.subscribe(topic, self.ipc_message_handler)
        payload = {"message": "Test IPC message"}
        message = MessageBuilder.build_from_config("IpcMessageTest",  "1.0", payload, self.config_mgr)
        correlation_id = message.get_header().correlation_id
        MessagingClient.publish(topic, message)
        time.sleep(1)
        self.assertIsNotNone(self.received_message)
        self.assertEqual(self.received_message.get_header().name, "IpcMessageTest")
        self.assertEqual(correlation_id, self.received_message.get_header().correlation_id)
        self.received_message = None

    def test_pub_sub_iot_core_message(self):
        topic = "test/testIotCoreTopic"
        MessagingClient.subscribe_to_iot_core(topic, self.iot_core_message_handler, QOS.AT_MOST_ONCE)
        payload = {"message": "Test IoT Core message"}
        message = MessageBuilder.build_from_config("IoTCoreMessageTest",  "1.0", payload, self.config_mgr)
        correlation_id = message.get_header().correlation_id
        MessagingClient.publish_to_iot_core(topic, message, QOS.AT_LEAST_ONCE)
        time.sleep(1)
        self.assertIsNotNone(self.received_message)
        self.assertEqual(self.received_message.get_header().name, "IoTCoreMessageTest")
        self.assertEqual(correlation_id, self.received_message.get_header().correlation_id)
        self.received_message = None

    def test_subscribe_with_filter(self):
        sub_topic = "test/+"
        pub_topic = "test/testIpcTopic"
        MessagingClient.subscribe(sub_topic, self.ipc_message_handler, 1)
        payload = {"message": "Test IPC message"}
        message = MessageBuilder.build_from_config("SubscribeWithFilterTest", "1.0", payload, self.config_mgr)
        correlation_id = message.get_header().correlation_id
        MessagingClient.publish(pub_topic, message)
        time.sleep(1)
        self.assertIsNotNone(self.received_message)
        self.assertEqual(self.received_message.get_header().name, "SubscribeWithFilterTest")
        self.assertEqual(correlation_id, self.received_message.get_header().correlation_id)
        self.received_message = None

    def test_request_reply_ipc(self):
        topic = "test/request"
        MessagingClient.subscribe(topic, self.request_handler, 1)
        payload = {"message": "Test Request Reply"}
        message = MessageBuilder.build_from_config("RequestTest", "1.0", payload, self.config_mgr)
        correlation_id = message.get_header().correlation_id
        success, reply = MessagingClient.request(topic, message).get(2)
        self.assertTrue(success)
        self.assertIsNotNone(reply)
        self.assertEqual(reply.get_header().name, "ReplyTest")
        self.assertEqual(correlation_id, reply.get_header().correlation_id)

    def test_request_reply_iot_core(self):
        topic = "test/iot_core_request"
        MessagingClient.subscribe_to_iot_core(topic, self.iot_core_request_handler, QOS.AT_MOST_ONCE, 1)
        payload = {"message": "Test Request Reply"}
        message = MessageBuilder.build_from_config("RequestTest", "1.0", payload, self.config_mgr)
        correlation_id = message.get_header().correlation_id
        success, reply = MessagingClient.request_from_iot_core(topic, message).get(2)
        self.assertTrue(success)
        self.assertIsNotNone(reply)
        self.assertEqual(reply.get_header().name, "ReplyTest")
        self.assertEqual(correlation_id, reply.get_header().correlation_id)

    def test_metric(self):
        from ggcommons.metrics.metric_emitter import MetricEmitter
        from ggcommons.metrics.metric import Metric
        from ggcommons.metrics.measure import Measure
        metric = Metric(thing_name=self.config_mgr.get_thing_name(),
                        component_name=self.config_mgr.get_component_name(),
                        name="performance")
        metric.add_measure(Measure("latency", "Milliseconds", 1))
        MetricEmitter.define_metric(metric)
        MetricEmitter.emit_metric_now("performance", {"latency": 1.123})
