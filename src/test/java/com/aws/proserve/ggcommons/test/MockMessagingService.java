/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.test;

import com.aws.proserve.ggcommons.interfaces.IMessagingService;
import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessageHandler;
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
}