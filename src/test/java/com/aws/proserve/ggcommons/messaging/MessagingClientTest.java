/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;
import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for MessagingClient class.
 * Tests static methods for messaging operations including IoT Core functionality.
 */
class MessagingClientTest {
    
    private TestMessagingProvider testProvider;
    
    @BeforeEach
    void setUp() {
        testProvider = new TestMessagingProvider();
        MessagingClient.messagingProvider = testProvider;
    }

    @Test
    void testPublishToIotCoreRaw() {
        JsonObject payload = new JsonObject();
        payload.addProperty("sensor", "temperature");
        payload.addProperty("value", 23.5);
        
        MessagingClient.publishToIotCoreRaw("sensors/temperature", payload, QOS.AT_LEAST_ONCE);
        
        assertTrue(testProvider.publishToIoTCoreRawCalled);
        assertEquals("sensors/temperature", testProvider.lastTopic);
        assertEquals(QOS.AT_LEAST_ONCE, testProvider.lastQos);
    }
    
    @Test
    void testPublishToIotCoreRawWithDifferentQos() {
        JsonObject payload = new JsonObject();
        payload.addProperty("alert", "critical");
        
        MessagingClient.publishToIotCoreRaw("alerts/critical", payload, QOS.AT_MOST_ONCE);
        
        assertTrue(testProvider.publishToIoTCoreRawCalled);
        assertEquals(QOS.AT_MOST_ONCE, testProvider.lastQos);
    }

    @Test
    void testCancelRequest() {
        ReplyFuture future = new ReplyFuture("test/reply/topic");
        
        MessagingClient.cancelRequest(future);
        
        assertTrue(testProvider.cancelRequestCalled);
        assertEquals(future, testProvider.lastCancelledFuture);
    }
    
    @Test
    void testCancelRequestWithDifferentTopic() {
        ReplyFuture future = new ReplyFuture("another/reply/topic");
        
        MessagingClient.cancelRequest(future);
        
        assertTrue(testProvider.cancelRequestCalled);
        assertEquals(future, testProvider.lastCancelledFuture);
    }

    @Test
    void testCancelRequestFromIoTCore() {
        ReplyFuture future = new ReplyFuture("iot/reply/topic");
        
        MessagingClient.cancelRequestFromIoTCore(future);
        
        assertTrue(testProvider.cancelRequestFromIoTCoreCalled);
        assertEquals(future, testProvider.lastCancelledIoTCoreFuture);
    }

    @Test
    void testReplyToIoTCore() {
        JsonObject requestPayload = new JsonObject();
        requestPayload.addProperty("command", "status");
        
        Message request = Message.build(requestPayload);
        request.header = new MessageHeader("StatusRequest", "1.0", "req-123");
        request.header.replyTo = "device/status/reply";
        
        JsonObject replyPayload = new JsonObject();
        replyPayload.addProperty("status", "online");
        
        Message reply = Message.build(replyPayload);
        reply.header = new MessageHeader("StatusReply", "1.0");
        
        MessagingClient.replyToIoTCore(request, reply);
        
        assertTrue(testProvider.replyToIoTCoreCalled);
        assertEquals(request, testProvider.lastRequest);
        assertEquals(reply, testProvider.lastReply);
    }

    @Test
    void testUnsubscribe() {
        String topicFilter = "sensors/+/temperature";
        
        MessagingClient.unsubscribe(topicFilter);
        
        assertTrue(testProvider.unsubscribeCalled);
        assertEquals(topicFilter, testProvider.lastUnsubscribeTopic);
    }
    
    @Test
    void testUnsubscribeWithWildcard() {
        String topicFilter = "alerts/#";
        
        MessagingClient.unsubscribe(topicFilter);
        
        assertTrue(testProvider.unsubscribeCalled);
        assertEquals(topicFilter, testProvider.lastUnsubscribeTopic);
    }

    @Test
    void testUnsubscribeFromIoTCore() {
        String topicFilter = "device/commands/+";
        
        MessagingClient.unsubscribeFromIoTCore(topicFilter);
        
        assertTrue(testProvider.unsubscribeFromIoTCoreCalled);
        assertEquals(topicFilter, testProvider.lastUnsubscribeIoTCoreTopic);
    }

    @Test
    void testTopicMatchesFilter() {
        // Test single-level wildcard
        assertTrue(MessagingClient.topicMatchesFilter("sensors/+/temperature", "sensors/device1/temperature"));
        assertTrue(MessagingClient.topicMatchesFilter("test/+", "test/data"));
        assertFalse(MessagingClient.topicMatchesFilter("test/+", "other/data"));
        
        // Test multi-level wildcard
        assertTrue(MessagingClient.topicMatchesFilter("sensors/#", "sensors/device1/temperature"));
        assertTrue(MessagingClient.topicMatchesFilter("test/#", "test/data/value"));
        assertTrue(MessagingClient.topicMatchesFilter("test/#", "test/data/value/nested"));
        assertFalse(MessagingClient.topicMatchesFilter("test/#", "other/data"));
        
        // Test exact match
        assertTrue(MessagingClient.topicMatchesFilter("exact/topic", "exact/topic"));
        assertFalse(MessagingClient.topicMatchesFilter("exact/topic", "different/topic"));
    }

    // Test helper class
    private static class TestMessagingProvider extends MessagingProvider {
        boolean publishToIoTCoreRawCalled = false;
        boolean cancelRequestCalled = false;
        boolean cancelRequestFromIoTCoreCalled = false;
        boolean replyToIoTCoreCalled = false;
        boolean unsubscribeCalled = false;
        boolean unsubscribeFromIoTCoreCalled = false;
        
        String lastTopic;
        QOS lastQos;
        ReplyFuture lastCancelledFuture;
        ReplyFuture lastCancelledIoTCoreFuture;
        Message lastRequest;
        Message lastReply;
        String lastUnsubscribeTopic;
        String lastUnsubscribeIoTCoreTopic;

        public TestMessagingProvider() {}

        @Override
        public void publish(String topic, Message message) {}

        @Override
        public void publishToIoTCore(String topic, Message message, QOS qos) {}

        @Override
        public void publishRaw(String topic, JsonObject payload) {}

        @Override
        public void publishToIoTCoreRaw(String topic, JsonObject payload, QOS qos) {
            publishToIoTCoreRawCalled = true;
            lastTopic = topic;
            lastQos = qos;
        }

        @Override
        public void subscribe(String topicFilter, java.util.function.BiConsumer<String, Message> callback, int maxConcurrency) {}

        @Override
        public void subscribeToIoTCore(String topicFilter, java.util.function.BiConsumer<String, Message> callback, QOS qos, int maxConcurrency) {}

        @Override
        public void unsubscribe(String topicFilter) {
            unsubscribeCalled = true;
            lastUnsubscribeTopic = topicFilter;
        }

        @Override
        public void unsubscribeFromIoTCore(String topicFilter) {
            unsubscribeFromIoTCoreCalled = true;
            lastUnsubscribeIoTCoreTopic = topicFilter;
        }

        @Override
        public ReplyFuture request(String topic, Message message) {
            return new ReplyFuture("test");
        }

        @Override
        public void cancelRequest(ReplyFuture future) {
            cancelRequestCalled = true;
            lastCancelledFuture = future;
        }

        @Override
        public void reply(Message request, Message reply) {}

        @Override
        public ReplyFuture requestFromIoTCore(String topic, Message request) {
            return new ReplyFuture("test");
        }

        @Override
        public void cancelRequestFromIoTCore(ReplyFuture future) {
            cancelRequestFromIoTCoreCalled = true;
            lastCancelledIoTCoreFuture = future;
        }

        @Override
        public void replyToIoTCore(Message request, Message reply) {
            replyToIoTCoreCalled = true;
            lastRequest = request;
            lastReply = reply;
        }

        @Override
        public Object getNativeLocalClient()
        {
            return null;
        }

        @Override
        public Object getNativeIotCoreClient()
        {
            return null;
        }


    }
}