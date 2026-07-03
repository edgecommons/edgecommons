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

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link Heartbeat} built via {@link HeartbeatBuilder} with the shared
 * test mocks (no broker / Nucleus / AWS). These exercise the previously-uncovered
 * {@code publishHeartbeat()} "messaging" branches:
 *
 * <ul>
 *   <li>destination {@code iot_core} -> {@code messagingService.publishToIoTCore(...)} (Heartbeat L153)</li>
 *   <li>an unrecognized destination -> the "Unrecognized messaging destination" warn branch (Heartbeat L157)</li>
 *   <li>{@code onConfigurationChanged()} -> re-init, which cancels/purges the existing timer
 *       (Heartbeat L74-75, L189-191)</li>
 * </ul>
 *
 * The heartbeat {@link java.util.Timer} fires its first task at delay 0, so constructing the
 * {@code Heartbeat} triggers {@code publishHeartbeat()} once synchronously-ish; we await briefly
 * for the published message and then {@code close()} the heartbeat.
 */
class HeartbeatPublishTest {

    /** A config whose heartbeat targets are messaging with the given destinations. */
    private static MockConfigurationService configWithMessagingTargets(String... destinations) {
        StringBuilder targets = new StringBuilder("[");
        for (int i = 0; i < destinations.length; i++) {
            if (i > 0) targets.append(',');
            targets.append("{\"type\":\"messaging\",\"config\":{\"destination\":\"")
                    .append(destinations[i])
                    .append("\",\"topic\":\"hb/{ThingName}/{ComponentName}\"}}");
        }
        targets.append("]");
        final String heartbeatJson =
                "{\"heartbeat\":{\"intervalSecs\":3600,\"targets\":" + targets + "}}";

        return new MockConfigurationService() {
            @Override
            public HeartbeatConfiguration getHeartbeatConfig() {
                JsonObject cfg = JsonParser.parseString(heartbeatJson).getAsJsonObject();
                return ConfigurationFactory.createHeartbeatConfiguration(cfg);
            }
        };
    }

    private static void awaitAtLeastOnePublish(MockMessagingService messaging) {
        for (int i = 0; i < 50 && messaging.getPublishedMessages().isEmpty(); i++) {
            try { Thread.sleep(20); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
        }
    }

    @Test
    void iotCoreDestinationPublishesToIotCore() {
        MockConfigurationService config = configWithMessagingTargets("iot_core");
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            awaitAtLeastOnePublish(messaging);
            assertFalse(messaging.getPublishedMessages().isEmpty(),
                    "iot_core target must publish a heartbeat via publishToIoTCore");
            // publishToIoTCore records a QOS; publish() (ipc) records null QOS.
            assertNotNull(messaging.getPublishedMessages().get(0).qos,
                    "iot_core publish must carry a QOS (publishToIoTCore path)");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void unrecognizedDestinationDoesNotPublish() {
        MockConfigurationService config = configWithMessagingTargets("totally_bogus_destination");
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            // Give the timer a moment to fire; the unrecognized branch only logs a warning.
            try { Thread.sleep(150); } catch (InterruptedException e) { Thread.currentThread().interrupt(); }
            assertTrue(messaging.getPublishedMessages().isEmpty(),
                    "an unrecognized destination must not publish anything");
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void onConfigurationChangedReinitializesTimer() {
        MockConfigurationService config = configWithMessagingTargets("iot_core");
        MockMessagingService messaging = new MockMessagingService();
        MockMetricService metrics = new MockMetricService();

        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(metrics)
                .build();
        try {
            // Re-init: cancels/purges the existing timer (L74-75) and reschedules (L189-191).
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
