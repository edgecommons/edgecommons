/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config.provider;

import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.ReplyFuture;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Behavior tests for {@link ConfigComponentProvider#loadConfiguration()} under the framework
 * request deadline (UNS-CANONICAL-DESIGN §5): the CONFIG_COMPONENT bootstrap request keeps its
 * 3-attempt retry contract — but because the deadline settles a timed-out request (reply
 * subscription unsubscribed, future completed exceptionally with {@link TimeoutException}), each
 * retry issues a FRESH request instead of re-awaiting the dead future.
 */
class ConfigComponentProviderTest {

    /** MockMessagingService whose request() futures are scripted per attempt. */
    private static final class ScriptedMessaging extends MockMessagingService {
        final List<ReplyFuture> scripted = new ArrayList<>();
        final AtomicInteger requests = new AtomicInteger();
        final AtomicInteger cancels = new AtomicInteger();

        @Override
        public ReplyFuture request(String topic, Message message) {
            int n = requests.getAndIncrement();
            return scripted.get(Math.min(n, scripted.size() - 1));
        }

        @Override
        public void cancelRequest(ReplyFuture replyFuture) {
            cancels.incrementAndGet();
            if (replyFuture.trySettle()) {
                replyFuture.complete(null);
            }
        }
    }

    private static ReplyFuture timedOut() {
        ReplyFuture f = new ReplyFuture("ggcommons/reply-deadline");
        f.trySettle();
        f.completeExceptionally(new TimeoutException("request timed out (framework deadline)"));
        return f;
    }

    private static ReplyFuture replied(JsonObject body) {
        // A real reply is a full message envelope; loadConfiguration reads its "body".
        JsonObject envelope = new JsonObject();
        envelope.add("body", body);
        ReplyFuture f = new ReplyFuture("ggcommons/reply-ok");
        f.trySettle();
        f.complete(MessageBuilder.fromObject(envelope));
        return f;
    }

    private static ConfigComponentProvider provider(ScriptedMessaging messaging) {
        return (ConfigComponentProvider) ConfigProviderBuilder.build(
                new MockConfigurationService(), "com.test.Comp", "thing",
                new String[]{"CONFIG_COMPONENT"}, messaging);
    }

    @Test
    void loadReturnsTheReplyBodyOnFirstAttempt() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        JsonObject cfg = new JsonObject();
        cfg.addProperty("component", "x");
        messaging.scripted.add(replied(cfg));

        JsonObject loaded = provider(messaging).loadConfiguration();

        assertEquals("x", loaded.get("component").getAsString());
        assertEquals(1, messaging.requests.get(), "one request suffices on the happy path");
    }

    @Test
    void frameworkDeadlineTimeoutsRetryWithFreshRequestsThenFailAfterThree() {
        // Every request's future was settled by the framework deadline (ExecutionException whose
        // cause is TimeoutException). The provider must retry with FRESH requests (the settled
        // future can never complete) and give up after 3 attempts.
        ScriptedMessaging messaging = new ScriptedMessaging();
        messaging.scripted.add(timedOut());
        messaging.scripted.add(timedOut());
        messaging.scripted.add(timedOut());

        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> provider(messaging).loadConfiguration());

        assertTrue(ex.getMessage().contains("3 tries"), "must report the 3-attempt failure: " + ex.getMessage());
        assertEquals(3, messaging.requests.get(), "each retry must issue a fresh request");
    }

    @Test
    void recoversWhenALaterAttemptSucceeds() {
        ScriptedMessaging messaging = new ScriptedMessaging();
        JsonObject cfg = new JsonObject();
        cfg.addProperty("component", "recovered");
        messaging.scripted.add(timedOut());
        messaging.scripted.add(timedOut());
        messaging.scripted.add(replied(cfg));

        JsonObject loaded = provider(messaging).loadConfiguration();

        assertEquals("recovered", loaded.get("component").getAsString());
        assertEquals(3, messaging.requests.get());
    }

    @Test
    void nonTimeoutExecutionFailuresStayFatal() {
        // A non-timeout ExecutionException (e.g. a transport error completing the future) must
        // keep the existing fatal contract — no retry.
        ScriptedMessaging messaging = new ScriptedMessaging();
        ReplyFuture broken = new ReplyFuture("ggcommons/reply-broken");
        broken.trySettle();
        broken.completeExceptionally(new IllegalStateException("transport exploded"));
        messaging.scripted.add(broken);

        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> provider(messaging).loadConfiguration());

        assertEquals(1, messaging.requests.get(), "a non-timeout failure must not be retried");
        assertTrue(ex.getMessage().contains("Failed to load configuration"));
    }
}
