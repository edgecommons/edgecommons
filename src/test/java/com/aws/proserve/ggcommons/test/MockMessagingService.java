/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.test;

import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageHandler;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;

/**
 * Mock implementation of IMessagingService for testing.
 */
public class MockMessagingService implements IMessagingService {
    private final List<PublishedMessage> publishedMessages = new ArrayList<>();
    private final Map<String, MessageHandler> subscriptions = new HashMap<>();
    
    public static class PublishedMessage {
        public final String topic;
        public final Message message;
        public final JsonObject rawPayload;
        public final QOS qos;
        
        public PublishedMessage(String topic, Message message, QOS qos) {
            this.topic = topic;
            this.message = message;
            this.rawPayload = null;
            this.qos = qos;
        }
        
        public PublishedMessage(String topic, JsonObject rawPayload) {
            this.topic = topic;
            this.message = null;
            this.rawPayload = rawPayload;
            this.qos = null;
        }
    }
    
    @Override
    public void subscribe(String topic, MessageHandler handler, int maxMessages) {
        subscriptions.put(topic, handler);
    }
    
    @Override
    public void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos) {
        subscriptions.put(topic, handler);
    }
    
    @Override
    public void subscribeToIoTCore(String topic, MessageHandler handler, QOS qos, int maxMessages) {
        subscriptions.put(topic, handler);
    }
    
    @Override
    public void publish(String topic, Message message) {
        publishedMessages.add(new PublishedMessage(topic, message, null));
    }
    
    @Override
    public void publishToIotCore(String topic, Message message, QOS qos) {
        publishedMessages.add(new PublishedMessage(topic, message, qos));
    }
    
    @Override
    public void publishRaw(String topic, JsonObject payload) {
        publishedMessages.add(new PublishedMessage(topic, payload));
    }
    
    @Override
    public CompletableFuture<Message> request(String topic, Message message) {
        publish(topic, message);
        return CompletableFuture.completedFuture(message); // Echo back for testing
    }
    
    @Override
    public CompletableFuture<Message> requestFromIoTCore(String topic, Message message) {
        publishToIotCore(topic, message, QOS.AT_LEAST_ONCE);
        return CompletableFuture.completedFuture(message); // Echo back for testing
    }
    
    @Override
    public void reply(Message originalMessage, Message replyMessage) {
        // For testing, we'll just record this as a published message
        publishedMessages.add(new PublishedMessage("reply", replyMessage, null));
    }
    
    // Test utility methods
    public List<PublishedMessage> getPublishedMessages() {
        return new ArrayList<>(publishedMessages);
    }
    
    public void clearPublishedMessages() {
        publishedMessages.clear();
    }
    
    public void simulateMessage(String topic, Message message) {
        MessageHandler handler = subscriptions.get(topic);
        if (handler != null) {
            handler.handle(topic, message);
        }
    }
    
    @Override
    public void subscribe(String topicFilter, MessageHandler handler) {
        subscriptions.put(topicFilter, handler);
    }
    
    @Override
    public void publishToIotCoreRaw(String topic, JsonObject payload, QOS qos) {
        publishedMessages.add(new PublishedMessage(topic, payload));
    }
    
    @Override
    public void unsubscribe(String topicFilter) {
        subscriptions.remove(topicFilter);
    }
    
    @Override
    public void unsubscribeFromIoTCore(String topicFilter) {
        subscriptions.remove(topicFilter);
    }
    
    @Override
    public void cancelRequest(ReplyFuture replyFuture) {
        // Mock implementation - no-op
    }
    
    @Override
    public void cancelRequestFromIoTCore(ReplyFuture replyFuture) {
        // Mock implementation - no-op
    }
    
    @Override
    public void replyToIoTCore(Message request, Message reply) {
        publishedMessages.add(new PublishedMessage("iot_core_reply", reply, null));
    }
    
    @Override
    public boolean topicMatchesFilter(String topicFilter, String topic) {
        // Simple mock implementation
        if (topicFilter.equals(topic)) return true;
        if (topicFilter.contains("+")) {
            String[] filterParts = topicFilter.split("/");
            String[] topicParts = topic.split("/");
            if (filterParts.length != topicParts.length) return false;
            for (int i = 0; i < filterParts.length; i++) {
                if (!filterParts[i].equals("+") && !filterParts[i].equals(topicParts[i])) {
                    return false;
                }
            }
            return true;
        }
        return false;
    }
    
    @Override
    public Object getNativeLocalClient() {
        return "MockLocalClient";
    }
    
    @Override
    public Object getNativeIotCoreClient() {
        return "MockIotCoreClient";
    }
    
    public void reset() {
        publishedMessages.clear();
        subscriptions.clear();
    }
}