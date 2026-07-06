/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the concrete (non-abstract) members of {@link MessagingProvider}: the
 * backward-compatible {@code subscribe}/{@code subscribeNorthbound} overloads that fill in
 * the default queue bound, the default no-op {@link MessagingProvider#close()}, and the
 * remaining {@link MessagingProvider#topicMatchesFilter} edge cases (literal char vs '/',
 * and the {@code parent/} + {@code parent/#} trailing-slash case).
 */
class MessagingProviderConcreteTest {

    /** Minimal test double recording the queue bound the overloads forward. */
    private static final class RecordingProvider extends MessagingProvider {
        int lastMaxMessages = Integer.MIN_VALUE;
        int lastMaxConcurrency = Integer.MIN_VALUE;
        Qos lastQos;

        @Override public void publish(String topic, Message message) { }
        @Override public void publishNorthbound(String topic, Message message, Qos qos) { }
        @Override public void publishRaw(String topic, JsonObject payload) { }
        @Override public void publishNorthboundRaw(String topic, JsonObject payload, Qos qos) { }

        @Override
        public void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                              int maxConcurrency, int maxMessages) {
            this.lastMaxConcurrency = maxConcurrency;
            this.lastMaxMessages = maxMessages;
        }

        @Override
        public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback, Qos qos,
                                       int maxConcurrency, int maxMessages) {
            this.lastMaxConcurrency = maxConcurrency;
            this.lastMaxMessages = maxMessages;
            this.lastQos = qos;
        }

        @Override public void unsubscribe(String topicFilter) { }
        @Override public void unsubscribeNorthbound(String topicFilter) { }
        @Override public ReplyFuture request(String topic, Message message) { return null; }
        @Override public ReplyFuture request(String topic, Message message, java.time.Duration timeout) { return null; }
        @Override public void cancelRequest(ReplyFuture future) { }
        @Override public void reply(Message request, Message reply) { }
        @Override public ReplyFuture requestNorthbound(String topic, Message request) { return null; }
        @Override public ReplyFuture requestNorthbound(String topic, Message request, java.time.Duration timeout) { return null; }
        @Override public void cancelRequestNorthbound(ReplyFuture future) { }
        @Override public void replyNorthbound(Message request, Message reply) { }
        @Override public Object getNativeLocalClient() { return null; }
        @Override public Object getNativeNorthboundClient() { return null; }
    }

    @Test
    void subscribeThreeArgOverloadUsesDefaultQueueBound() {
        RecordingProvider p = new RecordingProvider();
        p.subscribe("f/+", (t, m) -> { }, 3);
        assertEquals(3, p.lastMaxConcurrency);
        assertEquals(MessagingClient.DEFAULT_MAX_MESSAGES, p.lastMaxMessages);
    }

    @Test
    void subscribeNorthboundFourArgOverloadUsesDefaultQueueBound() {
        RecordingProvider p = new RecordingProvider();
        p.subscribeNorthbound("f/+", (t, m) -> { }, Qos.AT_LEAST_ONCE, 5);
        assertEquals(5, p.lastMaxConcurrency);
        assertEquals(MessagingClient.DEFAULT_MAX_MESSAGES, p.lastMaxMessages);
        assertEquals(Qos.AT_LEAST_ONCE, p.lastQos);
    }

    @Test
    void defaultCloseIsNoOp() {
        RecordingProvider p = new RecordingProvider();
        assertDoesNotThrow(p::close);
    }

    @Test
    void defaultConnectedIsFalse() {
        // A provider that does not report connectivity is treated as not-connected (not-ready).
        RecordingProvider p = new RecordingProvider();
        assertFalse(p.connected());
    }

    @Test
    void literalFilterCharVsTopicSeparatorDoesNotMatch() {
        // filter has a literal 'x' where the topic has a '/': must break (line 96) -> no match.
        assertFalse(MessagingProvider.topicMatchesFilter("ax", "a/"));
        assertFalse(MessagingProvider.topicMatchesFilter("ab", "a/b"));
    }

    @Test
    void parentWithTrailingSlashMatchesMultiLevelWildcard() {
        // 'sport/' (trailing slash) vs 'sport/#' exercises the topicPos-1 == '/' edge (lines 123-124).
        assertTrue(MessagingProvider.topicMatchesFilter("sport/#", "sport/"));
    }

    @Test
    void trailingSlashFilterPrefixEdge() {
        // Covers the '/#' startsWith fallback branch when filter has >1 trailing chars left.
        assertTrue(MessagingProvider.topicMatchesFilter("sport/#", "sport/x"));
    }
}
