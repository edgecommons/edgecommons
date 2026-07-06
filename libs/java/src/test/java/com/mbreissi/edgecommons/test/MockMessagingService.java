/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.test;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.messaging.Qos;

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
    // Simulated transport connectivity for the readiness model (FR-HB-2). Defaults to connected so a
    // freshly-built mock messaging client reports a ready transport; tests flip it to exercise the
    // disconnected -> not-ready path.
    private volatile boolean connected = true;
    // Records that the library closed messaging (the SIGTERM/shutdown teardown path).
    private volatile int closeCount = 0;

    public static class PublishedMessage {
        public final String topic;
        public final Message message;
        public final JsonObject rawPayload;
        public final Qos qos;
        /** Whether the publish came through the privileged {@code ReservedPublisher} seam. */
        public final boolean reserved;

        public PublishedMessage(String topic, Message message, Qos qos) {
            this(topic, message, qos, false);
        }

        public PublishedMessage(String topic, Message message, Qos qos, boolean reserved) {
            this.topic = topic;
            this.message = message;
            this.rawPayload = null;
            this.qos = qos;
            this.reserved = reserved;
        }

        public PublishedMessage(String topic, JsonObject rawPayload) {
            this(topic, rawPayload, false);
        }

        public PublishedMessage(String topic, JsonObject rawPayload, boolean reserved) {
            this.topic = topic;
            this.message = null;
            this.rawPayload = rawPayload;
            this.qos = null;
            this.reserved = reserved;
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
    public void subscribeNorthbound(String topic, BiConsumer<String, Message> handler, Qos qos) {
        subscriptions.put(topic, handler);
    }

    @Override
    public void subscribeNorthbound(String topic, BiConsumer<String, Message> handler, Qos qos, int maxConcurrency) {
        subscriptions.put(topic, handler);
    }

    @Override
    public void publish(String topic, Message message) {
        publishedMessages.add(new PublishedMessage(topic, message, null));
    }

    @Override
    public void publishNorthbound(String topic, Message message, Qos qos) {
        publishedMessages.add(new PublishedMessage(topic, message, qos));
    }

    @Override
    public void publishRaw(String topic, JsonObject payload) {
        publishedMessages.add(new PublishedMessage(topic, payload));
    }

    @Override
    public void publishNorthboundRaw(String topic, JsonObject payload, Qos qos) {
        publishedMessages.add(new PublishedMessage(topic, payload));
    }

    // The privileged ReservedPublisher seam delegates to these protected hooks; record them like
    // regular publishes (flagged reserved) so tests can assert the library publishers' output.

    @Override
    protected void publishReserved(String topic, Message message) {
        publishedMessages.add(new PublishedMessage(topic, message, null, true));
    }

    @Override
    protected void publishReservedRaw(String topic, JsonObject payload) {
        publishedMessages.add(new PublishedMessage(topic, payload, true));
    }

    @Override
    protected void publishReservedNorthbound(String topic, Message message, Qos qos) {
        publishedMessages.add(new PublishedMessage(topic, message, qos, true));
    }

    @Override
    public ReplyFuture request(String topic, Message message) {
        publish(topic, message);
        var future = new ReplyFuture(topic);
        future.complete(message); // Echo back for testing
        return future;
    }

    @Override
    public ReplyFuture requestNorthbound(String topic, Message message) {
        publishNorthbound(topic, message, Qos.AT_LEAST_ONCE);
        var future = new ReplyFuture(topic);
        future.complete(message); // Echo back for testing
        return future;
    }

    @Override
    public void reply(Message originalMessage, Message replyMessage) {
        // Mimic the real providers: stamp the request's correlation id onto the reply and
        // publish on the request's reply_to. Falls back to the legacy "reply" marker topic when
        // the request carries no reply_to (kept for existing assertions).
        String replyTo = originalMessage != null && originalMessage.getHeader() != null
                ? originalMessage.getHeader().getReplyTo() : null;
        if (originalMessage != null && originalMessage.getHeader() != null) {
            replyMessage.setCorrelationId(originalMessage.getHeader().getCorrelationId());
        }
        publishedMessages.add(new PublishedMessage(replyTo != null ? replyTo : "reply",
                replyMessage, null));
    }

    @Override
    public void replyNorthbound(Message request, Message reply) {
        publishedMessages.add(new PublishedMessage("iot_core_reply", reply, null));
    }

    @Override
    public void unsubscribe(String topicFilter) {
        subscriptions.remove(topicFilter);
    }

    @Override
    public void unsubscribeNorthbound(String topicFilter) {
        subscriptions.remove(topicFilter);
    }

    @Override
    public void cancelRequest(ReplyFuture replyFuture) {
        // Mock implementation - no-op
    }

    @Override
    public void cancelRequestNorthbound(ReplyFuture replyFuture) {
        // Mock implementation - no-op
    }

    @Override
    public boolean connected() {
        return connected;
    }

    /** Sets the simulated transport connectivity reported by {@link #connected()}. */
    public void setConnected(boolean connected) {
        this.connected = connected;
    }

    @Override
    public void close() {
        closeCount++;
    }

    /** Number of times the library invoked {@link #close()} (for idempotency assertions). */
    public int getCloseCount() {
        return closeCount;
    }

    @Override
    public Object getNativeLocalClient() {
        return "MockLocalClient";
    }

    @Override
    public Object getNativeNorthboundClient() {
        return "MockIotCoreClient";
    }

    // Test utility methods

    public List<PublishedMessage> getPublishedMessages() {
        return new ArrayList<>(publishedMessages);
    }

    public void clearPublishedMessages() {
        publishedMessages.clear();
    }

    /** The topic filters currently subscribed (for subscribe/unsubscribe lifecycle assertions). */
    public java.util.Set<String> getSubscribedTopics() {
        return new java.util.HashSet<>(subscriptions.keySet());
    }

    public void simulateMessage(String topic, Message message) {
        // Deliver to every subscription whose filter matches the topic (an exact-topic
        // subscription matches itself, so plain-topic tests behave as before; wildcard
        // subscriptions - e.g. the command inbox's ".../cmd/#" - receive concrete topics).
        for (Map.Entry<String, BiConsumer<String, Message>> sub
                : new HashMap<>(subscriptions).entrySet()) {
            if (sub.getKey().equals(topic)
                    || MessagingClient.topicMatchesFilter(sub.getKey(), topic)) {
                sub.getValue().accept(topic, message);
            }
        }
    }

    public void reset() {
        publishedMessages.clear();
        subscriptions.clear();
    }
}
