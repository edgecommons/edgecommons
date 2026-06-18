/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.aws.proserve.ggcommons.test.MockMessagingService;
import com.aws.proserve.ggcommons.test.MockMetricService;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Scheduling integration tests for {@link Heartbeat}. These validate the
 * {@link java.util.concurrent.ScheduledExecutorService}-based timer end-to-end:
 * that it actually fires periodically and that a configuration change dynamically
 * reschedules it (cancel the current task, reschedule on the same executor) — the
 * behavior the unit tests don't cover. No broker/AWS needed, but they rely on real
 * wall-clock timing with generous margins.
 */
class HeartbeatSchedulingTest {

    /** A MockConfigurationService whose heartbeat interval can be changed at runtime. */
    private static class MutableConfig extends MockConfigurationService {
        volatile int intervalSecs;

        MutableConfig(int intervalSecs) {
            this.intervalSecs = intervalSecs;
        }

        @Override
        public HeartbeatConfiguration getHeartbeatConfig() {
            String json = "{\"heartbeat\":{\"intervalSecs\":" + intervalSecs
                    + ",\"targets\":[{\"type\":\"messaging\",\"config\":"
                    + "{\"destination\":\"ipc\",\"topic\":\"hb/{ThingName}/{ComponentName}\"}}]}}";
            return ConfigurationFactory.createHeartbeatConfiguration(
                    JsonParser.parseString(json).getAsJsonObject());
        }
    }

    private static int sleepAndCount(MockMessagingService messaging, long millis) {
        try {
            Thread.sleep(millis);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return messaging.getPublishedMessages().size();
    }

    @Test
    void firesPeriodicallyAtConfiguredInterval() {
        MutableConfig config = new MutableConfig(1); // 1-second interval
        MockMessagingService messaging = new MockMessagingService();
        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            // scheduleAtFixedRate fires at ~0s, ~1s, ~2s -> at least 2 within ~2.5s.
            int count = sleepAndCount(messaging, 2500);
            assertTrue(count >= 2, "expected >=2 heartbeats in ~2.5s at a 1s interval, got " + count);
        } finally {
            heartbeat.close();
        }
    }

    @Test
    void configurationChangeReschedulesToNewInterval() {
        // Start slow: fires once at t=0, next not for an hour.
        MutableConfig config = new MutableConfig(3600);
        MockMessagingService messaging = new MockMessagingService();
        Heartbeat heartbeat = HeartbeatBuilder.create(config)
                .withMessagingService(messaging)
                .withMetricService(new MockMetricService())
                .build();
        try {
            int afterStart = sleepAndCount(messaging, 300); // the single t=0 fire
            // Speed it up; onConfigurationChanged must cancel the slow task and
            // reschedule a fast one on the SAME executor.
            config.intervalSecs = 1;
            assertTrue(heartbeat.onConfigurationChanged(), "onConfigurationChanged must return true");
            int afterReschedule = sleepAndCount(messaging, 2500);
            assertTrue(afterReschedule >= afterStart + 2,
                    "reschedule to 1s should add >=2 heartbeats; before=" + afterStart
                            + " after=" + afterReschedule);
        } finally {
            heartbeat.close();
        }
    }
}
