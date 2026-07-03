/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.heartbeat;

import com.mbreissi.ggcommons.config.ConfigurationFactory;
import com.mbreissi.ggcommons.config.HeartbeatConfiguration;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.mbreissi.ggcommons.test.MockMessagingService;
import com.mbreissi.ggcommons.test.MockMetricService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the reshaped {@link Heartbeat} (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20) with the
 * shared test mocks (no broker / Nucleus / AWS):
 *
 * <ul>
 *   <li>each tick publishes a {@code state} keepalive to the component's UNS state topic
 *       ({@code ecv1/{device}/{component}/main/state}) through the privileged
 *       {@code ReservedPublisher} seam — header name {@code "state"}, body
 *       {@code {"status":"RUNNING","uptimeSecs":n}};</li>
 *   <li>the measures are emitted as the metric {@code sys} through the metric subsystem;</li>
 *   <li>{@code destination: iotcore} routes the keepalive via {@code publishToIoTCore};</li>
 *   <li>{@code close()} publishes a best-effort {@code {"status":"STOPPED"}} state (once);</li>
 *   <li>{@code enabled: false} disables everything;</li>
 *   <li>no resolved identity -> the keepalive is skipped but the {@code sys} metric still flows.</li>
 * </ul>
 */
class HeartbeatPublishTest {

    /** The default mock identity's UNS state topic (device=test-thing, component=TestComponent). */
    private static final String STATE_TOPIC = "ecv1/test-thing/TestComponent/main/state";

    /** A config whose heartbeat section is the given §4.3-shape JSON (or "{}" for pure defaults). */
    private static MockConfigurationService configWithHeartbeat(String heartbeatJson) {
        final String json = "{\"heartbeat\":" + heartbeatJson + "}";
        return new MockConfigurationService() {
            @Override
            public HeartbeatConfiguration getHeartbeatConfig() {
                JsonObject cfg = JsonParser.parseString(json).getAsJsonObject();
                return ConfigurationFactory.createHeartbeatConfiguration(cfg);
            }
        };
    }

    private static void awaitAtLeastOnePublish(MockMessagingService messaging) {
        for (int i = 0; i < 100 && messaging.getPublishedMessages().isEmpty(); i++) {
            try { Thread.sleep(20); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
        }
    }

    private static void awaitAtLeastOneEmit(MockMetricService metrics) {
        for (int i = 0; i < 100 && metrics.getEmittedMetrics().isEmpty(); i++) {
            try { Thread.sleep(20); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
        }
    }

    @Test
    void publishesStateKeepaliveOnTheUnsStateTopic() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            awaitAtLeastOnePublish(messaging);
            List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
            assertFalse(published.isEmpty(), "the heartbeat must publish a state keepalive");
            MockMessagingService.PublishedMessage keepalive = published.get(0);
            assertEquals(STATE_TOPIC, keepalive.topic);
            assertTrue(keepalive.reserved,
                    "the state keepalive must go through the privileged ReservedPublisher seam");
            assertNull(keepalive.qos, "the default destination 'local' publishes locally (no QOS)");

            // Envelope: header name "state"; body {"status":"RUNNING","uptimeSecs":<n>}.
            assertEquals("state", keepalive.message.getHeader().getName());
            JsonObject body = keepalive.message.toDict().getAsJsonObject("body");
            assertEquals("RUNNING", body.get("status").getAsString());
            assertTrue(body.has("uptimeSecs"), "the RUNNING keepalive carries uptimeSecs");
            assertTrue(body.get("uptimeSecs").getAsLong() >= 0);
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void emitsTheMeasuresAsTheSysMetric() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            awaitAtLeastOneEmit(metrics);
            List<MockMetricService.EmittedMetric> emitted = metrics.getEmittedMetrics();
            assertFalse(emitted.isEmpty(), "the heartbeat must emit the measures as a metric");
            assertEquals("sys", emitted.get(0).name,
                    "the measures are the metric 'sys' (D6/D-U20), not 'heartbeat'");
            assertTrue(emitted.get(0).immediate, "the measures are emitted with emitMetricNow");
            // Default measures: cpu + memory.
            assertTrue(emitted.get(0).measureValues.containsKey("cpu_usage"));
            assertTrue(emitted.get(0).measureValues.containsKey("memory_usage"));
            // The 'sys' metric is defined up front too.
            assertTrue(metrics.isMetricDefined("sys"));
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void iotCoreDestinationPublishesTheKeepaliveToIotCore() {
        MockConfigurationService config =
                configWithHeartbeat("{\"intervalSecs\":3600,\"destination\":\"iotcore\"}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            awaitAtLeastOnePublish(messaging);
            List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
            assertFalse(published.isEmpty(), "iotcore destination must still publish the keepalive");
            assertEquals(STATE_TOPIC, published.get(0).topic);
            assertNotNull(published.get(0).qos,
                    "destination iotcore must publish via publishToIoTCore (carries a QOS)");
            assertTrue(published.get(0).reserved);
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void closePublishesABestEffortStoppedStateOnce() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        awaitAtLeastOnePublish(messaging);
        messaging.clearPublishedMessages();

        heartbeat.close();
        heartbeat.close(); // idempotent - the STOPPED state must go out at most once

        List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
        assertEquals(1, published.size(), "close() must publish the STOPPED state exactly once");
        assertEquals(STATE_TOPIC, published.get(0).topic);
        assertTrue(published.get(0).reserved);
        JsonObject body = published.get(0).message.toDict().getAsJsonObject("body");
        assertEquals("STOPPED", body.get("status").getAsString());
        assertFalse(body.has("uptimeSecs"), "the STOPPED state body is {\"status\":\"STOPPED\"}");
    }

    @Test
    void disabledHeartbeatPublishesNothing() {
        MockConfigurationService config = configWithHeartbeat("{\"enabled\":false}");
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            try { Thread.sleep(200); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
            assertTrue(messaging.getPublishedMessages().isEmpty(),
                    "enabled:false must not publish a state keepalive");
            assertTrue(metrics.getEmittedMetrics().isEmpty(),
                    "enabled:false must not emit the sys metric");
        } finally {
            heartbeat.close();
        }
        // And close() after a disabled run must NOT publish a STOPPED state (nothing was running).
        assertTrue(messaging.getPublishedMessages().isEmpty());
    }

    @Test
    void missingIdentitySkipsTheKeepaliveButKeepsTheSysMetric() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        config.setComponentIdentity(null); // the test/subclass bring-up case
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            awaitAtLeastOneEmit(metrics);
            assertFalse(metrics.getEmittedMetrics().isEmpty(),
                    "the sys metric must still flow without a resolved identity");
            assertTrue(messaging.getPublishedMessages().isEmpty(),
                    "no resolved identity -> no UNS state topic -> keepalive skipped");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void publishStateNowReEmitsTheRunningKeepaliveOutOfBand() {
        // The republish-state re-announce seam (DESIGN-uns §9.3/§9.4): same payload and seam as a
        // periodic tick, emitted immediately and in addition to the schedule.
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            awaitAtLeastOnePublish(messaging); // the startup tick
            messaging.clearPublishedMessages();

            heartbeat.publishStateNow();

            List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
            assertEquals(1, published.size(), "publishStateNow must emit exactly one keepalive");
            assertEquals(STATE_TOPIC, published.get(0).topic);
            assertTrue(published.get(0).reserved,
                    "the re-announce must go through the privileged ReservedPublisher seam");
            JsonObject body = published.get(0).message.toDict().getAsJsonObject("body");
            assertEquals("RUNNING", body.get("status").getAsString());
            assertTrue(body.has("uptimeSecs"), "the re-announce is the RUNNING keepalive payload");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void publishStateNowRespectsHeartbeatDisabled() {
        // heartbeat.enabled=false opts the component out of the state surface - the broadcast
        // re-announce must not re-enable it.
        MockConfigurationService config = configWithHeartbeat("{\"enabled\":false}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            heartbeat.publishStateNow();
            assertTrue(messaging.getPublishedMessages().isEmpty(),
                    "enabled:false must suppress the out-of-band re-announce too");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void onConfigurationChangedReinitializesTimer() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            assertTrue(heartbeat.onConfigurationChanged(),
                    "onConfigurationChanged must return true");
            awaitAtLeastOnePublish(messaging);
            assertFalse(messaging.getPublishedMessages().isEmpty(),
                    "after re-init the heartbeat must still publish");
        } finally {
            heartbeat.close();
        }
    }
}
