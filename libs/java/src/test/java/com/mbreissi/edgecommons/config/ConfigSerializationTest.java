/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Regression tests for configuration {@code toDict()} serialization.
 */
class ConfigSerializationTest {

    /**
     * The cloudwatch target's toDict() previously serialized the (null) {@code topic} field
     * into {@code intervalSecs}. It must serialize the actual interval.
     */
    @Test
    void cloudwatchToDictWritesIntervalSecs() {
        JsonObject cfg = JsonParser.parseString(
                "{\"target\":\"cloudwatch\",\"targetConfig\":{\"intervalSecs\":30}}").getAsJsonObject();
        MetricConfiguration mc = new MetricConfiguration(cfg);

        JsonObject tc = mc.toDict().getAsJsonObject("targetConfig");
        assertEquals(30, tc.get("intervalSecs").getAsInt());
    }

    /**
     * Heartbeat toDict() previously (a) wrote the measures block under "metric" while the
     * constructor reads "measures", and (b) copy-pasted includeDisk into threads/files.
     * It must round-trip cleanly.
     */
    @Test
    void heartbeatToDictRoundTripsMeasures() {
        JsonObject cfg = JsonParser.parseString(
                "{\"intervalSecs\":7,\"measures\":{\"cpu\":true,\"memory\":false,\"threads\":true,\"files\":false}}")
                .getAsJsonObject();
        HeartbeatConfiguration hb = new HeartbeatConfiguration(cfg);

        JsonObject dict = hb.toDict();
        assertTrue(dict.has("measures"), "heartbeat toDict() must use the 'measures' key");
        JsonObject m = dict.getAsJsonObject("measures");
        assertTrue(m.get("cpu").getAsBoolean());
        assertEquals(false, m.get("memory").getAsBoolean());
        assertTrue(m.get("threads").getAsBoolean(), "threads must reflect includeThreads, not includeDisk");
        assertEquals(false, m.get("files").getAsBoolean(), "files must reflect includeFiles, not includeDisk");

        // Round-trip: feeding toDict() back into the constructor yields identical measures.
        HeartbeatConfiguration hb2 = new HeartbeatConfiguration(dict);
        assertEquals(m, hb2.toDict().getAsJsonObject("measures"));
    }
}
