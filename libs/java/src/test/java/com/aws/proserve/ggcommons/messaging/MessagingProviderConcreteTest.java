/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import software.amazon.awssdk.aws.greengrass.model.QOS;

import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the concrete (non-abstract) members of {@link MessagingProvider}: the
 * backward-compatible {@code subscribe}/{@code subscribeToIoTCore} overloads that fill in
 * the default queue bound, the default no-op {@link MessagingProvider#close()}, and the
 * remaining {@link MessagingProvider#topicMatchesFilter} edge cases (literal char vs '/',
 * and the {@code parent/} + {@code parent/#} trailing-slash case).
 */
class MessagingProviderConcreteTest {

    /** Minimal test double recording the queue bound the overloads forward. */
    private static final class RecordingProvider extends MessagingProvider {
        int lastMaxMessages = Integer.MIN_VALUE;
        int lastMaxConcurrency = Integer.MIN_VALUE;
        QOS lastQos;

        @Override public void publish(String topic, Message message) { }
        @Override public void publishToIoTCore(String topic, Message message, QOS qos) { }
        @Override public void publishRaw(String topic, JsonObject payload) { }
        @Override public void publishToIoTCoreRaw(String topic, JsonObject payload, QOS qos) { }

        @Override
        public void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                              int maxConcurrency, int maxMessages) {
            this.lastMaxConcurrency = maxConcurrency;
            this.lastMaxMessages = maxMessages;
        }

        @Override
        public void subscribeToIoTCore(String topicFilter, BiConsumer<String, Message> callback, QOS qos,
                                       int maxConcurrency, int maxMessages) {
            this.lastMaxConcurrency = maxConcurrency;
            this.lastMaxMessages = maxMessages;
            this.lastQos = qos;
        }

        @Override public void unsubscribe(String topicFilter) { }
        @Override public void unsubscribeFromIoTCore(String topicFilter) { }
        @Override public ReplyFuture request(String topic, Message message) { return null; }
        @Override public void cancelRequest(ReplyFuture future) { }
        @Override public void reply(Message request, Message reply) { }
        @Override public ReplyFuture requestFromIoTCore(String topic, Message request) { return null; }
        @Override public void cancelRequestFromIoTCore(ReplyFuture future) { }
        @Override public void replyToIoTCore(Message request, Message reply) { }
        @Override public Object getNativeLocalClient() { return null; }
        @Override public Object getNativeIotCoreClient() { return null; }
    }

    @Test
    void subscribeThreeArgOverloadUsesDefaultQueueBound() {
        RecordingProvider p = new RecordingProvider();
        p.subscribe("f/+", (t, m) -> { }, 3);
        assertEquals(3, p.lastMaxConcurrency);
        assertEquals(MessagingClient.DEFAULT_MAX_MESSAGES, p.lastMaxMessages);
    }

    @Test
    void subscribeToIoTCoreFourArgOverloadUsesDefaultQueueBound() {
        RecordingProvider p = new RecordingProvider();
        p.subscribeToIoTCore("f/+", (t, m) -> { }, QOS.AT_LEAST_ONCE, 5);
        assertEquals(5, p.lastMaxConcurrency);
        assertEquals(MessagingClient.DEFAULT_MAX_MESSAGES, p.lastMaxMessages);
        assertEquals(QOS.AT_LEAST_ONCE, p.lastQos);
    }

    @Test
    void defaultCloseIsNoOp() {
        RecordingProvider p = new RecordingProvider();
        assertDoesNotThrow(p::close);
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
