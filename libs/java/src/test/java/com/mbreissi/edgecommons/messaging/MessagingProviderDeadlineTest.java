/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.time.Duration;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the framework-owned {@code request()} deadline machinery on the
 * {@link MessagingProvider} base class (UNS-CANONICAL-DESIGN §5, D-U5): the effective-timeout
 * resolution (per-call wins, zero disables, config default late-bound), the shared lazy
 * single-thread daemon scheduler, and the single idempotent settle path (§5.1) — the deadline
 * cleans up and completes exceptionally even when the caller never awaits the future, and
 * reply/cancel/deadline race safely with exactly one winner.
 *
 * <p>This is the broker-free seam: the timer is armed directly via the protected
 * {@code armRequestDeadline} on a no-op provider subclass, with the cleanup Runnable standing in
 * for the provider-specific unsubscribe + pending-entry removal.
 */
class MessagingProviderDeadlineTest {

    /** Minimal no-op provider exposing the base-class deadline machinery. */
    private static final class NoopProvider extends MessagingProvider {
        @Override public void publish(String topic, Message message) { }
        @Override public void publishNorthbound(String topic, Message message, Qos qos) { }
        @Override public void publishRaw(String topic, JsonObject payload) { }
        @Override public void publishNorthboundRaw(String topic, JsonObject payload, Qos qos) { }
        @Override public void subscribe(String topicFilter, BiConsumer<String, Message> callback,
                                        int maxConcurrency, int maxMessages) { }
        @Override public void subscribeNorthbound(String topicFilter, BiConsumer<String, Message> callback,
                                                 Qos qos, int maxConcurrency, int maxMessages) { }
        @Override public void unsubscribe(String topicFilter) { }
        @Override public void unsubscribeNorthbound(String topicFilter) { }
        @Override public ReplyFuture request(String topic, Message message) { return null; }
        @Override public ReplyFuture request(String topic, Message message, Duration timeout) { return null; }
        @Override public void cancelRequest(ReplyFuture future) { }
        @Override public void reply(Message request, Message reply) { }
        @Override public ReplyFuture requestNorthbound(String topic, Message request) { return null; }
        @Override public ReplyFuture requestNorthbound(String topic, Message request, Duration timeout) { return null; }
        @Override public void cancelRequestNorthbound(ReplyFuture future) { }
        @Override public void replyNorthbound(Message request, Message reply) { }
        @Override public Object getNativeLocalClient() { return null; }
        @Override public Object getNativeNorthboundClient() { return null; }
    }

    // --- effective-timeout resolution ------------------------------------------------------------

    @Test
    void builtInDefaultIsThirtySeconds() {
        NoopProvider p = new NoopProvider();
        assertEquals(Duration.ofSeconds(MessagingProvider.DEFAULT_REQUEST_TIMEOUT_SECONDS),
                p.getDefaultRequestTimeout());
        assertEquals(Duration.ofSeconds(30), p.effectiveRequestTimeout(null),
                "no per-call value -> the built-in 30 s default applies (pre-late-bind behavior)");
    }

    @Test
    void perCallValueWinsOverDefault() {
        NoopProvider p = new NoopProvider();
        p.setDefaultRequestTimeout(Duration.ofSeconds(60));
        assertEquals(Duration.ofMillis(250), p.effectiveRequestTimeout(Duration.ofMillis(250)));
    }

    @Test
    void perCallZeroDisablesTheDeadlineForThatCall() {
        NoopProvider p = new NoopProvider();
        p.setDefaultRequestTimeout(Duration.ofSeconds(60));
        assertNull(p.effectiveRequestTimeout(Duration.ZERO),
                "explicit Duration.ZERO must disable the deadline even when a default is set");
    }

    @Test
    void lateBoundDefaultApplies() {
        NoopProvider p = new NoopProvider();
        p.setDefaultRequestTimeout(Duration.ofSeconds(7));  // the EdgeCommons late-bind path
        assertEquals(Duration.ofSeconds(7), p.effectiveRequestTimeout(null));
    }

    @Test
    void zeroOrNullDefaultDisables() {
        NoopProvider p = new NoopProvider();
        p.setDefaultRequestTimeout(Duration.ZERO);   // messaging.requestTimeoutSeconds: 0
        assertNull(p.effectiveRequestTimeout(null));
        p.setDefaultRequestTimeout(null);
        assertNull(p.effectiveRequestTimeout(null));
    }

    @Test
    void negativeResolvesToDisabled() {
        NoopProvider p = new NoopProvider();
        assertNull(p.effectiveRequestTimeout(Duration.ofSeconds(-1)));
    }

    // --- the deadline itself ---------------------------------------------------------------------

    @Test
    void deadlineFiresCleansUpAndCompletesExceptionallyWithoutEverAwaiting() throws Exception {
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-neverawaited");
        AtomicInteger cleanups = new AtomicInteger();

        p.armRequestDeadline(future, Duration.ofMillis(100), cleanups::incrementAndGet);

        // The caller NEVER calls get(): poll only observer methods. The framework timer must still
        // run the cleanup and complete the future exceptionally (the reply-subscription leak fix).
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while (!future.isDone() && System.nanoTime() < deadline) {
            Thread.sleep(10);
        }
        assertTrue(future.isDone(), "deadline never completed the un-awaited future");
        assertTrue(future.isCompletedExceptionally());
        assertEquals(1, cleanups.get(), "cleanup (unsubscribe + pending removal) must run exactly once");
        assertTrue(future.isSettled());

        ExecutionException ex = assertThrows(ExecutionException.class, future::get);
        assertInstanceOf(TimeoutException.class, ex.getCause());
        assertTrue(ex.getCause().getMessage().contains("edgecommons/reply-neverawaited"),
                "timeout message should name the reply topic");
        p.close();
    }

    @Test
    void replyBeforeDeadlineCancelsTimerAndCleanupNeverRuns() throws Exception {
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-early");
        AtomicInteger cleanups = new AtomicInteger();
        p.armRequestDeadline(future, Duration.ofMillis(150), cleanups::incrementAndGet);

        // Simulate reply arrival winning the settle race (the provider arrival path).
        assertTrue(future.trySettle(), "the reply must win the settle CAS");
        future.complete(Message.build("reply"));

        // Wait well past the deadline: the timer was canceled on settle, so the cleanup must not
        // run and the future must not flip to exceptional.
        Thread.sleep(400);
        assertFalse(future.isCompletedExceptionally(), "settled request must not be timed out later");
        assertEquals(0, cleanups.get(), "deadline cleanup must not run after the reply settled");
        assertNotNull(future.get(1, TimeUnit.SECONDS));
        p.close();
    }

    @Test
    void deadlineThenStragglerReplyIsIdempotent() throws Exception {
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-straggler");
        AtomicInteger cleanups = new AtomicInteger();
        p.armRequestDeadline(future, Duration.ofMillis(50), cleanups::incrementAndGet);

        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while (!future.isDone() && System.nanoTime() < deadline) {
            Thread.sleep(10);
        }
        assertTrue(future.isCompletedExceptionally());

        // A straggler reply after the deadline settled: loses the CAS -> the provider drops it.
        assertFalse(future.trySettle(), "straggler must lose the settle CAS");
        // Even a direct complete() cannot overwrite the exceptional completion.
        assertFalse(future.complete(Message.build("late")));
        assertTrue(future.isCompletedExceptionally());
        assertEquals(1, cleanups.get(), "no double cleanup");
        p.close();
    }

    @Test
    void cancelThenDeadlineDoesNotDoubleCleanup() throws Exception {
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-cancel");
        AtomicInteger cleanups = new AtomicInteger();
        p.armRequestDeadline(future, Duration.ofMillis(100), cleanups::incrementAndGet);

        // cancelRequest path: wins the settle, cancels the timer, completes with null.
        assertTrue(future.trySettle());
        future.complete(null);

        Thread.sleep(300);
        assertEquals(0, cleanups.get(), "deadline must not fire after cancel settled");
        assertNull(future.get(1, TimeUnit.SECONDS));
        p.close();
    }

    @Test
    void disabledTimeoutArmsNothing() throws Exception {
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-disabled");
        AtomicInteger cleanups = new AtomicInteger();

        p.armRequestDeadline(future, p.effectiveRequestTimeout(Duration.ZERO), cleanups::incrementAndGet);

        Thread.sleep(200);
        assertFalse(future.isDone(), "no deadline may fire when the timeout is disabled");
        assertEquals(0, cleanups.get());
        p.close();
    }

    @Test
    void closeShutsDownThePendingDeadline() throws Exception {
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-close");
        AtomicInteger cleanups = new AtomicInteger();
        p.armRequestDeadline(future, Duration.ofMillis(200), cleanups::incrementAndGet);

        p.close();  // shuts the shared scheduler down; the armed task must not fire afterwards

        Thread.sleep(500);
        assertEquals(0, cleanups.get(), "no deadline may fire after the provider closed");
        assertFalse(future.isDone());
    }

    @Test
    void schedulerIsSharedAndDeadlinesRunConcurrently() throws Exception {
        // Two requests on one provider share the single deadline thread; both must settle.
        NoopProvider p = new NoopProvider();
        ReplyFuture f1 = new ReplyFuture("r1");
        ReplyFuture f2 = new ReplyFuture("r2");
        AtomicInteger cleanups = new AtomicInteger();
        p.armRequestDeadline(f1, Duration.ofMillis(50), cleanups::incrementAndGet);
        p.armRequestDeadline(f2, Duration.ofMillis(80), cleanups::incrementAndGet);

        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while ((!f1.isDone() || !f2.isDone()) && System.nanoTime() < deadline) {
            Thread.sleep(10);
        }
        assertTrue(f1.isCompletedExceptionally());
        assertTrue(f2.isCompletedExceptionally());
        assertEquals(2, cleanups.get());
        p.close();
    }

    @Test
    void aFailingCleanupStillTimesOutTheCaller() throws Exception {
        // The cleanup is provider-supplied (unsubscribe + pending-entry removal). If it blows up,
        // the caller must still be released with a TimeoutException — a broken unsubscribe must
        // never strand a request on a reply that is no longer coming.
        NoopProvider p = new NoopProvider();
        ReplyFuture future = new ReplyFuture("edgecommons/reply-badcleanup");

        p.armRequestDeadline(future, Duration.ofMillis(50), () -> {
            throw new IllegalStateException("unsubscribe failed");
        });

        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while (!future.isDone() && System.nanoTime() < deadline) {
            Thread.sleep(10);
        }
        assertTrue(future.isCompletedExceptionally(),
                "a failing cleanup must not suppress the deadline");
        ExecutionException ex = assertThrows(ExecutionException.class, future::get);
        assertInstanceOf(TimeoutException.class, ex.getCause());
        p.close();
    }

    @Test
    void aRequestArmedWhileTheProviderIsClosingProceedsWithoutADeadline() throws Exception {
        // Shutdown races an in-flight request: the deadline scheduler is already gone, so no timer
        // can be armed. Arming must degrade to "no deadline" rather than throwing the shutdown's
        // RejectedExecutionException back at the caller.
        NoopProvider p = new NoopProvider();
        ReplyFuture warmUp = new ReplyFuture("edgecommons/reply-warmup");
        p.armRequestDeadline(warmUp, Duration.ofMillis(20), () -> { });
        p.close();

        ReplyFuture late = new ReplyFuture("edgecommons/reply-late");
        AtomicInteger cleanups = new AtomicInteger();
        assertDoesNotThrow(() -> p.armRequestDeadline(late, Duration.ofMillis(20),
                cleanups::incrementAndGet));

        Thread.sleep(150);
        assertFalse(late.isDone(), "no deadline could be armed, so nothing times the request out");
        assertEquals(0, cleanups.get());
    }
}
