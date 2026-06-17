/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.test;

import com.aws.proserve.ggcommons.messaging.Message;
import com.aws.proserve.ggcommons.messaging.MessagingClient;
import com.aws.proserve.ggcommons.messaging.ReplyFuture;
import com.google.gson.JsonObject;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.function.BiConsumer;

/**
 * Mock MessagingClient for testing. Records published messages and stored subscriptions
 * instead of talking to a real broker / IPC. Extends {@link MessagingClient} (no real
 * provider is created) so it can be injected wherever a MessagingClient is expected.
 */
public class MockMessagingService extends MessagingClient {
    private final List<PublishedMessage> publishedMessages = new ArrayList<>();
    private final Map<String, BiConsumer<String, Message>> subscriptions = new HashMap<>();

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
    public void subscribe(String topic, BiConsumer<String, Message> handler, int maxConcurrency) {
        subscriptions.put(topic, handler);
    }

    @Override
    public void subscribe(String topic, BiConsumer<String, Message> handler) {
        subscriptions.put(topic, handler);
    }

    @Override
    public void subscribeToIoTCore(String topic, BiConsumer<String, Message> handler, QOS qos) {
        subscriptions.put(topic, handler);
    }

    @Override
    public void subscribeToIoTCore(String topic, BiConsumer<String, Message> handler, QOS qos, int maxConcurrency) {
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
    public void publishToIotCoreRaw(String topic, JsonObject payload, QOS qos) {
        publishedMessages.add(new PublishedMessage(topic, payload));
    }

    @Override
    public ReplyFuture request(String topic, Message message) {
        publish(topic, message);
        ReplyFuture future = new ReplyFuture(topic);
        future.complete(message); // Echo back for testing
        return future;
    }

    @Override
    public ReplyFuture requestFromIoTCore(String topic, Message message) {
        publishToIotCore(topic, message, QOS.AT_LEAST_ONCE);
        ReplyFuture future = new ReplyFuture(topic);
        future.complete(message); // Echo back for testing
        return future;
    }

    @Override
    public void reply(Message originalMessage, Message replyMessage) {
        publishedMessages.add(new PublishedMessage("reply", replyMessage, null));
    }

    @Override
    public void replyToIoTCore(Message request, Message reply) {
        publishedMessages.add(new PublishedMessage("iot_core_reply", reply, null));
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
    public Object getNativeLocalClient() {
        return "MockLocalClient";
    }

    @Override
    public Object getNativeIotCoreClient() {
        return "MockIotCoreClient";
    }

    // Test utility methods

    public List<PublishedMessage> getPublishedMessages() {
        return new ArrayList<>(publishedMessages);
    }

    public void clearPublishedMessages() {
        publishedMessages.clear();
    }

    public void simulateMessage(String topic, Message message) {
        BiConsumer<String, Message> handler = subscriptions.get(topic);
        if (handler != null) {
            handler.accept(topic, message);
        }
    }

    public void reset() {
        publishedMessages.clear();
        subscriptions.clear();
    }
}
