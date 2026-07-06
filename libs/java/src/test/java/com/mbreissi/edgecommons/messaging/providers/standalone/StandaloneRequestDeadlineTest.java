/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.standalone;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessagingConfiguration;
import com.mbreissi.edgecommons.messaging.ReplyFuture;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.google.gson.Gson;
import com.google.gson.JsonObject;
import io.moquette.broker.Server;
import io.moquette.broker.config.MemoryConfig;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import java.net.ServerSocket;
import java.nio.file.Files;
import java.time.Duration;
import java.util.Properties;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

import static org.junit.jupiter.api.Assertions.*;

/**
 * End-to-end tests of the framework-owned {@code request()} deadline (UNS-CANONICAL-DESIGN §5)
 * against an in-process Moquette broker: the deadline fires at send-time + timeout and — even when
 * the caller never awaits the future — unsubscribes the ephemeral reply topic and removes the
 * pending entry (the reply-subscription leak fix); the per-call overload wins over the provider
 * default; {@code Duration.ZERO} disables; reply-then-deadline and deadline-then-cancel are
 * idempotent (single settle, no double unsubscribe / completion).
 */
class StandaloneRequestDeadlineTest {

    private static Server broker;
    private static int port;
    private static final MockConfigurationService MOCK_CONFIG = new MockConfigurationService();
    private StandaloneMessagingProvider provider;

    @BeforeAll
    static void startBroker() throws Exception {
        try (ServerSocket s = new ServerSocket(0)) {
            port = s.getLocalPort();
        }
        Properties props = new Properties();
        props.setProperty("host", "127.0.0.1");
        props.setProperty("port", String.valueOf(port));
        props.setProperty("allow_anonymous", "true");
        props.setProperty("persistence_enabled", "false");
        props.setProperty("data_path", Files.createTempDirectory("moquette-deadline").toString() + "/");
        broker = new Server();
        broker.startServer(new MemoryConfig(props));
    }

    @AfterAll
    static void stopBroker() {
        if (broker != null) {
            broker.stopServer();
        }
    }

    @AfterEach
    void closeProvider() {
        if (provider != null) {
            provider.close();
            provider = null;
        }
    }

    private StandaloneMessagingProvider localProvider(String clientId) {
        String json = """
                { "messaging": { "local": { "host": "127.0.0.1", "port": %d, "clientId": "%s" } } }"""
                .formatted(port, clientId);
        MessagingConfiguration cfg = new Gson().fromJson(json, MessagingConfiguration.class);
        return new StandaloneMessagingProvider(cfg, "test-thing");
    }

    private Message msg(String name) {
        JsonObject payload = new JsonObject();
        payload.addProperty("k", "v");
        return MessageBuilder.create(name, "1.0").withPayload(payload).withConfig(MOCK_CONFIG).build();
    }

    /** Polls until the future is done or the guard elapses — WITHOUT ever calling get(). */
    private static void awaitDone(ReplyFuture future, long guardMillis) throws InterruptedException {
        long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(guardMillis);
        while (!future.isDone() && System.nanoTime() < deadline) {
            Thread.sleep(10);
        }
    }

    @Test
    void neverAwaitedRequestTimesOutAndCleansUpTheReplySubscription() throws Exception {
        provider = localProvider("deadline-leak");
        // No responder on this topic; short per-call deadline. The caller never calls get().
        ReplyFuture future = provider.request("deadline/no-responder", msg("Q"), Duration.ofMillis(300));
        String replyTopic = future.replyTopic;
        assertTrue(provider.hasLocalSubscription(replyTopic), "reply subscription must exist at send time");
        assertTrue(provider.hasPendingRequest(replyTopic), "pending entry must exist at send time");

        awaitDone(future, 5000);

        // The core leak fix: the framework timer settled the request even though the future was
        // never awaited — reply subscription gone, pending entry gone, future exceptional.
        assertTrue(future.isDone(), "deadline never fired");
        assertTrue(future.isCompletedExceptionally(), "deadline must complete the future exceptionally");
        assertFalse(provider.hasLocalSubscription(replyTopic), "reply subscription must be unsubscribed");
        assertFalse(provider.hasPendingRequest(replyTopic), "pending entry must be removed");

        ExecutionException ex = assertThrows(ExecutionException.class, future::get);
        assertInstanceOf(TimeoutException.class, ex.getCause());
    }

    @Test
    void perCallTimeoutWinsOverProviderDefault() throws Exception {
        provider = localProvider("deadline-percall");
        provider.setDefaultRequestTimeout(Duration.ofSeconds(60));  // long default
        long start = System.nanoTime();
        ReplyFuture future = provider.request("deadline/percall", msg("Q"), Duration.ofMillis(200));

        awaitDone(future, 5000);

        long elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start);
        assertTrue(future.isCompletedExceptionally(), "per-call 200 ms deadline must fire");
        assertTrue(elapsedMs < 30_000, "the explicit per-call value must win over the 60 s default");
    }

    @Test
    void configuredDefaultAppliesWhenNoPerCallTimeoutIsGiven() throws Exception {
        provider = localProvider("deadline-default");
        provider.setDefaultRequestTimeout(Duration.ofMillis(200));  // the late-bind path
        ReplyFuture future = provider.request("deadline/default", msg("Q"));  // 2-arg request()

        awaitDone(future, 5000);

        assertTrue(future.isCompletedExceptionally(), "the bound default deadline must apply to request(topic, msg)");
        assertFalse(provider.hasLocalSubscription(future.replyTopic));
        assertFalse(provider.hasPendingRequest(future.replyTopic));
    }

    @Test
    void zeroPerCallTimeoutDisablesTheDeadline() throws Exception {
        provider = localProvider("deadline-zero");
        provider.setDefaultRequestTimeout(Duration.ofMillis(150));  // short default that must NOT apply
        ReplyFuture future = provider.request("deadline/zero", msg("Q"), Duration.ZERO);

        Thread.sleep(600);  // well past the (disabled) default
        assertFalse(future.isDone(), "Duration.ZERO must disable the deadline for this call");
        assertTrue(provider.hasLocalSubscription(future.replyTopic), "subscription must remain while pending");

        // Clean up via cancelRequest: settles, unsubscribes, completes with null.
        provider.cancelRequest(future);
        assertTrue(future.isDone());
        assertNull(future.get(2, TimeUnit.SECONDS));
        assertFalse(provider.hasLocalSubscription(future.replyTopic));
        assertFalse(provider.hasPendingRequest(future.replyTopic));
    }

    @Test
    void replyBeforeDeadlineCompletesNormallyAndDeadlineNoOps() throws Exception {
        provider = localProvider("deadline-reply-first");
        provider.subscribe("deadline/echo", (t, request) -> provider.reply(request, msg("Reply")), 1, -1);

        ReplyFuture future = provider.request("deadline/echo", msg("Q"), Duration.ofMillis(800));
        Message reply = future.get(5, TimeUnit.SECONDS);
        assertEquals("Reply", reply.getHeader().getName());
        assertFalse(provider.hasLocalSubscription(future.replyTopic), "arrival settle must unsubscribe once");
        assertFalse(provider.hasPendingRequest(future.replyTopic));

        // Wait past the armed deadline: the settled request must not flip to exceptional and the
        // (canceled) timer must not have run a second cleanup — unsubscribe of the already-removed
        // filter would be a no-op, but the future state is the observable contract.
        Thread.sleep(1200);
        assertFalse(future.isCompletedExceptionally(), "a replied request must never be timed out afterwards");
        assertEquals("Reply", future.get().getHeader().getName());
    }

    @Test
    void cancelAfterDeadlineIsANoOp() throws Exception {
        provider = localProvider("deadline-cancel-late");
        ReplyFuture future = provider.request("deadline/cancel-late", msg("Q"), Duration.ofMillis(150));
        awaitDone(future, 5000);
        assertTrue(future.isCompletedExceptionally());

        // cancelRequest after the deadline settled: loses the CAS, must not overwrite the
        // exceptional completion with null and must not double-unsubscribe.
        provider.cancelRequest(future);
        assertTrue(future.isCompletedExceptionally(), "cancel must not overwrite the timeout completion");
    }

    @Test
    void cancelBeforeDeadlinePreservesExistingContract() throws Exception {
        provider = localProvider("deadline-cancel-early");
        ReplyFuture future = provider.request("deadline/cancel-early", msg("Q"), Duration.ofSeconds(30));
        provider.cancelRequest(future);

        assertTrue(future.isDone());
        assertNull(future.get(2, TimeUnit.SECONDS), "cancelRequest keeps its complete(null) contract");
        assertFalse(provider.hasLocalSubscription(future.replyTopic));
        assertFalse(provider.hasPendingRequest(future.replyTopic));

        // The canceled request's timer is gone: nothing may fire later.
        Thread.sleep(300);
        assertFalse(future.isCompletedExceptionally());
    }
}
