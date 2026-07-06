/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.heartbeat;

import com.mbreissi.edgecommons.config.ConfigurationFactory;
import com.mbreissi.edgecommons.config.HeartbeatConfiguration;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.test.MockMetricService;
import com.google.gson.JsonArray;
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

    @Test
    void stateKeepaliveCarriesPerInstanceConnectivityFromTheProvider() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            awaitAtLeastOnePublish(messaging); // startup tick — no provider yet
            JsonObject startup = messaging.getPublishedMessages().get(0).message.toDict().getAsJsonObject("body");
            assertFalse(startup.has("instances"), "no provider -> no instances[] section");

            messaging.clearPublishedMessages();
            heartbeat.setInstanceConnectivityProvider(() -> List.of(
                    InstanceConnectivity.of("filler1", true, "opc.tcp://kep:49320"),
                    InstanceConnectivity.of("kep2", false)));
            heartbeat.publishStateNow();

            JsonObject body = messaging.getPublishedMessages().get(0).message.toDict().getAsJsonObject("body");
            assertEquals("RUNNING", body.get("status").getAsString());
            JsonArray instances = body.getAsJsonArray("instances");
            assertEquals(2, instances.size());
            JsonObject first = instances.get(0).getAsJsonObject();
            assertEquals("filler1", first.get("instance").getAsString());
            assertTrue(first.get("connected").getAsBoolean());
            assertEquals("opc.tcp://kep:49320", first.get("detail").getAsString());
            JsonObject second = instances.get(1).getAsJsonObject();
            assertEquals("kep2", second.get("instance").getAsString());
            assertFalse(second.get("connected").getAsBoolean());
            assertFalse(second.has("detail"), "no detail -> omitted");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void aThrowingConnectivityProviderNeverSuppressesTheKeepalive() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            awaitAtLeastOnePublish(messaging);
            messaging.clearPublishedMessages();
            heartbeat.setInstanceConnectivityProvider(() -> { throw new RuntimeException("boom"); });
            heartbeat.publishStateNow();

            List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
            assertEquals(1, published.size(), "a throwing provider must not suppress the keepalive");
            JsonObject body = published.get(0).message.toDict().getAsJsonObject("body");
            assertEquals("RUNNING", body.get("status").getAsString());
            assertFalse(body.has("instances"), "a throwing provider omits instances[]");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void anEmptyOrNullProviderResultOmitsTheInstancesSection() {
        MockConfigurationService config = configWithHeartbeat("{\"intervalSecs\":3600}");
        MockMessagingService messaging = new MockMessagingService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            awaitAtLeastOnePublish(messaging);

            heartbeat.setInstanceConnectivityProvider(List::of); // empty list
            messaging.clearPublishedMessages();
            heartbeat.publishStateNow();
            assertFalse(messaging.getPublishedMessages().get(0).message.toDict()
                    .getAsJsonObject("body").has("instances"), "empty list -> no instances[]");

            heartbeat.setInstanceConnectivityProvider(() -> null); // null result
            messaging.clearPublishedMessages();
            heartbeat.publishStateNow();
            assertFalse(messaging.getPublishedMessages().get(0).message.toDict()
                    .getAsJsonObject("body").has("instances"), "null result -> no instances[]");

            heartbeat.setInstanceConnectivityProvider(null); // cleared
            messaging.clearPublishedMessages();
            heartbeat.publishStateNow();
            assertFalse(messaging.getPublishedMessages().get(0).message.toDict()
                    .getAsJsonObject("body").has("instances"), "cleared provider -> no instances[]");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void instanceConnectivitySerializesAndValidates() {
        JsonObject json = InstanceConnectivity.of("plc1", true, "tcp://10.0.0.50:502").toJson();
        assertEquals("plc1", json.get("instance").getAsString());
        assertTrue(json.get("connected").getAsBoolean());
        assertEquals("tcp://10.0.0.50:502", json.get("detail").getAsString());

        assertFalse(InstanceConnectivity.of("plc1", false).toJson().has("detail"),
                "no detail -> omitted");
        assertFalse(new InstanceConnectivity("plc1", false, "  ").toJson().has("detail"),
                "blank detail -> omitted");

        InstanceConnectivity c = new InstanceConnectivity("srv", true, "d");
        assertEquals("srv", c.getInstance());
        assertTrue(c.isConnected());
        assertEquals("d", c.getDetail());

        assertThrows(IllegalArgumentException.class, () -> new InstanceConnectivity(null, true, null));
        assertThrows(IllegalArgumentException.class, () -> new InstanceConnectivity("  ", true, null));
    }
}
