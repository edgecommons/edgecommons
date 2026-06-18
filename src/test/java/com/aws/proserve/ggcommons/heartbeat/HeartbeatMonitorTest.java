/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.heartbeat;

import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.config.HeartbeatConfiguration;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link HeartbeatMonitor#getStats()}. The monitor reads real process
 * metrics via OSHI; these tests assert only the JSON structure produced for various
 * measure selections (which sub-objects are present), not specific metric values.
 */
class HeartbeatMonitorTest {

    private static HeartbeatConfiguration heartbeatConfigFromJson(String json) {
        JsonObject hb = JsonParser.parseString(json).getAsJsonObject();
        // ConfigurationFactory expects the heartbeat block nested under "heartbeat".
        JsonObject wrapper = new JsonObject();
        wrapper.add("heartbeat", hb);
        return ConfigurationFactory.createHeartbeatConfiguration(wrapper);
    }

    @Test
    void getStatsWithCpuAndMemoryEnabled() {
        HeartbeatConfiguration config = heartbeatConfigFromJson(
                "{\"measures\":{\"cpu\":true,\"memory\":true,\"threads\":false,\"files\":false}}");
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertTrue(stats.has("cpu"));
        assertTrue(stats.getAsJsonObject("cpu").has("cpu_usage"));
        assertTrue(stats.has("memory"));
        assertTrue(stats.getAsJsonObject("memory").has("memory_usage"));
        assertFalse(stats.has("threads"));
        assertFalse(stats.has("files"));
    }

    @Test
    void getStatsWithThreadsAndFilesEnabled() {
        HeartbeatConfiguration config = heartbeatConfigFromJson(
                "{\"measures\":{\"cpu\":false,\"memory\":false,\"threads\":true,\"files\":true}}");
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertFalse(stats.has("cpu"));
        assertFalse(stats.has("memory"));
        assertTrue(stats.has("threads"));
        assertTrue(stats.getAsJsonObject("threads").has("threads"));
        assertTrue(stats.has("files"));
        assertTrue(stats.getAsJsonObject("files").has("files"));
    }

    @Test
    void getStatsWithDefaultConfigIncludesCpuAndMemoryOnly() {
        // null JSON -> defaults: cpu=true, memory=true, threads=false, files=false, disk=false
        HeartbeatConfiguration config = new HeartbeatConfiguration(null);
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertTrue(stats.has("cpu"));
        assertTrue(stats.has("memory"));
        assertFalse(stats.has("threads"));
        assertFalse(stats.has("files"));
        // disk is never supported in ggcommons-java, so it is always absent
        assertFalse(stats.has("disk"));
    }

    @Test
    void getStatsWithAllDisableableMeasuresOff() {
        HeartbeatConfiguration config = heartbeatConfigFromJson(
                "{\"measures\":{\"cpu\":false,\"memory\":false,\"threads\":false,\"files\":false}}");
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        JsonObject stats = monitor.getStats();

        assertNotNull(stats);
        assertEquals(0, stats.size(), "no measures enabled -> empty stats object");
    }

    @Test
    void updateMetricsCanBeCalledRepeatedly() {
        HeartbeatConfiguration config = new HeartbeatConfiguration(null);
        HeartbeatMonitor monitor = new HeartbeatMonitor(config);

        // Two consecutive snapshots should not throw; exercises previous/current swap.
        assertDoesNotThrow(monitor::updateMetrics);
        assertDoesNotThrow(monitor::updateMetrics);
        assertNotNull(monitor.getStats());
    }
}
