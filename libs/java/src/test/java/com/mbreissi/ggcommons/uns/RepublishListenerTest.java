/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.uns;

import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.LongSupplier;
import java.util.function.LongUnaryOperator;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Deterministic unit tests for the {@link RepublishListener} (DESIGN-uns §9.3/§9.4, the late-join
 * lever) via the injected delayer/clock/jitter seams — no sleeping, no real scheduler:
 *
 * <ul>
 *   <li>{@code start()} subscribes both own-device {@code _bcast} republish topics on the primary
 *       connection (exact rootless topics, built with the {@code _bcast} pseudo-component);</li>
 *   <li>{@code republish-state} re-runs the state action; {@code republish-cfg} the cfg action
 *       (verb separation);</li>
 *   <li>the jitter window ({@value RepublishListener#JITTER_WINDOW_MS} ms) is passed to the
 *       injected jitter source and the returned delay is what gets scheduled;</li>
 *   <li>a broadcast while a re-announce is pending — or within the
 *       {@value RepublishListener#COOLDOWN_MS} ms cooldown of the last accepted trigger —
 *       coalesces (no amplification); the verbs rate-limit independently;</li>
 *   <li>foreign/malformed payloads (wrong header name, raw no-header envelope, null) are ignored
 *       and never throw;</li>
 *   <li>{@code close()} unsubscribes both topics, drops pending re-announces and is
 *       idempotent; a missing resolved identity disables the listener.</li>
 * </ul>
 */
class RepublishListenerTest {

    /** The default mock identity's device is {@code test-thing} (single 'device' level). */
    private static final String STATE_TOPIC = "ecv1/test-thing/_bcast/main/cmd/republish-state";
    private static final String CFG_TOPIC = "ecv1/test-thing/_bcast/main/cmd/republish-cfg";

    /** Records scheduled (task, delay) pairs; the test runs tasks synchronously on demand. */
    private static final class RecordingDelayer implements RepublishListener.Delayer {
        final List<Runnable> tasks = new ArrayList<>();
        final List<Long> delays = new ArrayList<>();

        @Override
        public void schedule(Runnable task, long delayMillis) {
            tasks.add(task);
            delays.add(delayMillis);
        }

        /** Runs and clears every scheduled task (the "jitter delay elapsed" step). */
        void runAll() {
            List<Runnable> toRun = new ArrayList<>(tasks);
            tasks.clear();
            delays.clear();
            toRun.forEach(Runnable::run);
        }
    }

    private MockConfigurationService config;
    private MockMessagingService messaging;
    private RecordingDelayer delayer;
    private AtomicLong clock;
    private AtomicLong jitterWindowSeen;
    private long nextJitter;
    private AtomicInteger stateRepublishes;
    private AtomicInteger cfgRepublishes;
    private RepublishListener listener;

    @BeforeEach
    void setUp() {
        config = new MockConfigurationService();
        messaging = new MockMessagingService();
        delayer = new RecordingDelayer();
        clock = new AtomicLong(0);
        jitterWindowSeen = new AtomicLong(-1);
        nextJitter = 0;
        stateRepublishes = new AtomicInteger();
        cfgRepublishes = new AtomicInteger();
        LongSupplier clockMillis = clock::get;
        LongUnaryOperator jitter = window -> {
            jitterWindowSeen.set(window);
            return nextJitter;
        };
        listener = new RepublishListener(config, messaging,
                stateRepublishes::incrementAndGet, cfgRepublishes::incrementAndGet,
                delayer, clockMillis, jitter);
    }

    private static Message broadcast(String verb) {
        return MessageBuilder.create(verb, "1.0").withPayload(new JsonObject()).build();
    }

    @Test
    void startSubscribesBothOwnDeviceBcastTopics() {
        listener.start();
        assertEquals(Set.of(STATE_TOPIC, CFG_TOPIC), messaging.getSubscribedTopics(),
                "start() must subscribe exactly the two own-device _bcast republish topics");
    }

    @Test
    void republishStateReEmitsTheStateKeepalive() {
        listener.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertEquals(0, stateRepublishes.get(), "the re-announce must wait for the jitter delay");
        delayer.runAll();
        assertEquals(1, stateRepublishes.get(), "republish-state must re-run the state action");
        assertEquals(0, cfgRepublishes.get(), "republish-state must not touch the cfg action");
    }

    @Test
    void republishCfgReRunsTheEffectiveConfigPublisher() {
        listener.start();
        messaging.simulateMessage(CFG_TOPIC, broadcast("republish-cfg"));
        delayer.runAll();
        assertEquals(1, cfgRepublishes.get(), "republish-cfg must re-run the cfg action");
        assertEquals(0, stateRepublishes.get(), "republish-cfg must not touch the state action");
    }

    @Test
    void jitterWindowIsAppliedToTheScheduledDelay() {
        nextJitter = 1234;
        listener.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertEquals(RepublishListener.JITTER_WINDOW_MS, jitterWindowSeen.get(),
                "the jitter source must be asked for a delay within the normative window");
        assertEquals(List.of(1234L), delayer.delays,
                "the scheduled delay must be exactly the jittered value");
    }

    @Test
    void broadcastsCoalesceWhileAReAnnounceIsPending() {
        listener.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertEquals(1, delayer.tasks.size(),
                "a looping broadcast must coalesce to a single pending re-announce");
        delayer.runAll();
        assertEquals(1, stateRepublishes.get());
    }

    @Test
    void broadcastsCoalesceWithinTheCooldownAndAcceptAfterIt() {
        listener.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        delayer.runAll(); // fired; cooldown runs from the ACCEPTED trigger at t=0

        clock.set(RepublishListener.COOLDOWN_MS - 1);
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertTrue(delayer.tasks.isEmpty(), "a broadcast inside the cooldown must coalesce");
        assertEquals(1, stateRepublishes.get());

        clock.set(RepublishListener.COOLDOWN_MS);
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertEquals(1, delayer.tasks.size(), "the cooldown boundary must accept again");
        delayer.runAll();
        assertEquals(2, stateRepublishes.get());
    }

    @Test
    void theVerbsRateLimitIndependently() {
        listener.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        // With a state re-announce pending, a cfg broadcast must still be accepted.
        messaging.simulateMessage(CFG_TOPIC, broadcast("republish-cfg"));
        assertEquals(2, delayer.tasks.size(), "state and cfg coalesce/cooldown independently");
        delayer.runAll();
        assertEquals(1, stateRepublishes.get());
        assertEquals(1, cfgRepublishes.get());
    }

    @Test
    void foreignAndMalformedPayloadsAreIgnored() {
        listener.start();
        // Wrong verb name in the header (foreign command on the topic).
        messaging.simulateMessage(STATE_TOPIC, broadcast("something-else"));
        // A raw (headerless) envelope - e.g. junk JSON published on the broadcast topic.
        messaging.simulateMessage(STATE_TOPIC, MessageBuilder.fromObject(new JsonObject()));
        // A null message must not crash the callback either.
        assertDoesNotThrow(() -> messaging.simulateMessage(STATE_TOPIC, null));
        assertTrue(delayer.tasks.isEmpty(), "foreign/malformed payloads must never schedule");
        assertEquals(0, stateRepublishes.get());
        assertEquals(0, cfgRepublishes.get());
    }

    @Test
    void aFailingReAnnounceIsSwallowedAndDoesNotWedgeTheVerb() {
        RepublishListener failing = new RepublishListener(config, messaging,
                () -> { throw new RuntimeException("boom"); }, cfgRepublishes::incrementAndGet,
                delayer, clock::get, window -> 0);
        failing.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertDoesNotThrow(delayer::runAll, "an action failure must be swallowed");
        // After the cooldown the verb accepts again (pending was cleared despite the failure).
        clock.set(RepublishListener.COOLDOWN_MS);
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertEquals(1, delayer.tasks.size());
        failing.close();
    }

    @Test
    void closeUnsubscribesBothTopicsAndDropsPendingReAnnounces() {
        listener.start();
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        listener.close();
        assertTrue(messaging.getSubscribedTopics().isEmpty(),
                "close() must unsubscribe both _bcast topics (unsubscribe-before-exit)");
        delayer.runAll();
        assertEquals(0, stateRepublishes.get(),
                "a pending re-announce must not fire after close()");
        // And a late broadcast (e.g. a stale queued delivery) is ignored.
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertTrue(delayer.tasks.isEmpty());
    }

    @Test
    void closeIsIdempotentAndStartAfterCloseIsANoOp() {
        listener.start();
        listener.close();
        assertDoesNotThrow(listener::close);
        listener.start(); // closed -> must not resubscribe
        assertTrue(messaging.getSubscribedTopics().isEmpty());
    }

    @Test
    void startIsIdempotent() {
        listener.start();
        listener.start();
        assertEquals(Set.of(STATE_TOPIC, CFG_TOPIC), messaging.getSubscribedTopics());
        messaging.simulateMessage(STATE_TOPIC, broadcast("republish-state"));
        assertEquals(1, delayer.tasks.size(), "a double start must not double-schedule");
    }

    @Test
    void missingIdentityDisablesTheListener() {
        config.setComponentIdentity(null); // the mock/test bring-up case
        listener.start();
        assertTrue(messaging.getSubscribedTopics().isEmpty(),
                "no resolved identity -> no _bcast subscriptions (WARN + disabled)");
        assertDoesNotThrow(listener::close);
    }

    @Test
    void productionConstructorSchedulesForReal() throws Exception {
        // The production wiring (owned scheduler + real clock + real jitter) end to end: the
        // jittered delay is bounded by the window, so the re-announce lands within it.
        RepublishListener production = new RepublishListener(config, messaging,
                stateRepublishes::incrementAndGet, cfgRepublishes::incrementAndGet);
        production.start();
        assertEquals(Set.of(STATE_TOPIC, CFG_TOPIC), messaging.getSubscribedTopics());
        messaging.simulateMessage(CFG_TOPIC, broadcast("republish-cfg"));
        long deadline = System.currentTimeMillis() + RepublishListener.JITTER_WINDOW_MS + 3_000;
        while (cfgRepublishes.get() == 0 && System.currentTimeMillis() < deadline) {
            Thread.sleep(20);
        }
        assertEquals(1, cfgRepublishes.get(),
                "the production scheduler must fire the re-announce within the jitter window");
        production.close();
        assertTrue(messaging.getSubscribedTopics().isEmpty());
    }
}
