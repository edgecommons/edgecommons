/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.heartbeat;

import com.breissinger.ggcommons.config.ConfigurationFactory;
import com.breissinger.ggcommons.config.HeartbeatConfiguration;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the {@link HeartbeatMonitor#getStats()} measure branches the existing
 * {@code HeartbeatMonitorTest} leaves uncovered: {@code disk} ({@code getDiskUsage},
 * HeartbeatMonitor L58, L106-L115) and {@code fds} ({@code getFdCount}, L70, L125-L135).
 *
 * <p>The monitor reads real filesystem/OS metrics; these tests assert only the JSON
 * structure (which sub-objects and keys appear), not the values, so they are
 * deterministic across platforms.
 */
class HeartbeatMonitorMeasuresTest {

    private static HeartbeatConfiguration heartbeatConfigFromJson(String json) {
        JsonObject hb = JsonParser.parseString(json).getAsJsonObject();
        JsonObject wrapper = new JsonObject();
        wrapper.add("heartbeat", hb);
        return ConfigurationFactory.createHeartbeatConfiguration(wrapper);
    }

    @Test
    void getStatsWithDiskEnabledEmitsDiskTotals() {
        HeartbeatConfiguration config = heartbeatConfigFromJson(
                "{\"measures\":{\"cpu\":false,\"memory\":false,\"disk\":true}}");
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertTrue(stats.has("disk"), "disk=true must add a 'disk' sub-object");
        JsonObject disk = stats.getAsJsonObject("disk");
        assertTrue(disk.has("disk_total"));
        assertTrue(disk.has("disk_used"));
        assertTrue(disk.has("disk_free"));
        // total == used + free by construction (used = total - free).
        double total = disk.get("disk_total").getAsDouble();
        double used = disk.get("disk_used").getAsDouble();
        double free = disk.get("disk_free").getAsDouble();
        assertEquals(total, used + free, 1.0e-6, "disk_total must equal disk_used + disk_free");
        assertTrue(total >= 0.0);
    }

    @Test
    void getStatsWithFdsEnabledEmitsFdsCount() {
        HeartbeatConfiguration config = heartbeatConfigFromJson(
                "{\"measures\":{\"cpu\":false,\"memory\":false,\"fds\":true}}");
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertTrue(stats.has("fds"), "fds=true must add an 'fds' sub-object");
        JsonObject fds = stats.getAsJsonObject("fds");
        assertTrue(fds.has("fds"));
        // On Unix the count is >= 0; on Windows the MXBean is absent and the monitor
        // deliberately reports -1 ("unavailable") rather than a bogus value.
        long fdCount = fds.get("fds").getAsLong();
        assertTrue(fdCount >= 0L || fdCount == -1L,
                "fds must be a non-negative count or -1 (unavailable on Windows)");
    }

    @Test
    void getStatsWithEveryMeasureEnabledIncludesAllSubObjects() {
        HeartbeatConfiguration config = heartbeatConfigFromJson(
                "{\"measures\":{\"cpu\":true,\"memory\":true,\"disk\":true,"
                        + "\"threads\":true,\"files\":true,\"fds\":true}}");
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertTrue(stats.has("cpu"));
        assertTrue(stats.has("memory"));
        assertTrue(stats.has("disk"));
        assertTrue(stats.has("threads"));
        assertTrue(stats.has("files"));
        assertTrue(stats.has("fds"));
    }
}
