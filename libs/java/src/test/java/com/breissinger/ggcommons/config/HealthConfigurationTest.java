/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link HealthConfiguration} parsing and the {@link ConfigurationFactory} accessor —
 * defaults, explicit {@code enabled} (both values + the "explicitly set" distinction that drives the
 * enablement precedence), custom port/paths, and the defensive out-of-range port fallback.
 */
class HealthConfigurationTest {

    @Test
    void nullSectionYieldsDefaultsAndEnabledUnset() {
        HealthConfiguration hc = new HealthConfiguration(null);
        assertFalse(hc.isEnabledExplicitlySet(), "absent section -> enabled not explicitly set");
        assertFalse(hc.isEnabled());
        assertEquals(HealthConfiguration.DEFAULT_PORT, hc.getPort());
        assertEquals(HealthConfiguration.DEFAULT_LIVENESS_PATH, hc.getLivenessPath());
        assertEquals(HealthConfiguration.DEFAULT_READINESS_PATH, hc.getReadinessPath());
        assertEquals(HealthConfiguration.DEFAULT_STARTUP_PATH, hc.getStartupPath());
    }

    @Test
    void emptyObjectYieldsDefaultsAndEnabledUnset() {
        HealthConfiguration hc = new HealthConfiguration(new JsonObject());
        assertFalse(hc.isEnabledExplicitlySet());
        assertEquals(8081, hc.getPort());
    }

    @Test
    void explicitEnabledTrueIsParsedAndMarkedExplicit() {
        HealthConfiguration hc = parse("{ \"enabled\": true }");
        assertTrue(hc.isEnabledExplicitlySet());
        assertTrue(hc.isEnabled());
    }

    @Test
    void explicitEnabledFalseIsParsedAndMarkedExplicit() {
        HealthConfiguration hc = parse("{ \"enabled\": false }");
        assertTrue(hc.isEnabledExplicitlySet());
        assertFalse(hc.isEnabled());
    }

    @Test
    void customPortAndPathsAreParsed() {
        HealthConfiguration hc = parse("""
                { "port": 9090, "livenessPath": "/l", "readinessPath": "/r", "startupPath": "/s" }""");
        assertEquals(9090, hc.getPort());
        assertEquals("/l", hc.getLivenessPath());
        assertEquals("/r", hc.getReadinessPath());
        assertEquals("/s", hc.getStartupPath());
        // enabled omitted -> not explicitly set even though other keys are present.
        assertFalse(hc.isEnabledExplicitlySet());
    }

    @Test
    void outOfRangePortFallsBackToDefault() {
        assertEquals(8081, parse("{ \"port\": 0 }").getPort());
        assertEquals(8081, parse("{ \"port\": 70000 }").getPort());
    }

    @Test
    void factoryReadsHealthSectionFromFullConfig() {
        JsonObject full = JsonParser.parseString("""
                { "component": {}, "health": { "enabled": true, "port": 8088 } }""").getAsJsonObject();
        HealthConfiguration hc = ConfigurationFactory.createHealthConfiguration(full);
        assertTrue(hc.isEnabled());
        assertEquals(8088, hc.getPort());
    }

    @Test
    void factoryDefaultsWhenSectionAbsent() {
        JsonObject full = JsonParser.parseString("{ \"component\": {} }").getAsJsonObject();
        HealthConfiguration hc = ConfigurationFactory.createHealthConfiguration(full);
        assertFalse(hc.isEnabledExplicitlySet());
        assertEquals(8081, hc.getPort());
    }

    private static HealthConfiguration parse(String json) {
        return new HealthConfiguration(JsonParser.parseString(json).getAsJsonObject());
    }
}
