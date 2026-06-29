/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link MetricConfiguration} focused on the prometheus-target additions (FR-MET-1):
 * the {@code targetConfig.port}/{@code path} parsing with schema defaults, and the
 * {@code targetExplicitlySet} flag that drives the FR-RT-3 metric-target precedence.
 */
class MetricConfigurationTest {

    private static MetricConfiguration of(String metricJson) {
        JsonObject root = new JsonObject();
        root.add("metricEmission", JsonParser.parseString(metricJson).getAsJsonObject());
        return ConfigurationFactory.createMetricConfiguration(root);
    }

    @Test
    void defaultsWhenNoMetricEmissionSection() {
        MetricConfiguration cfg = ConfigurationFactory.createMetricConfiguration(null);
        assertEquals("log", cfg.getTarget());
        assertFalse(cfg.isTargetExplicitlySet());
        assertEquals(9090, cfg.getPrometheusPort());
        assertEquals("/metrics", cfg.getPrometheusPath());
    }

    @Test
    void explicitTargetSetsTheFlag() {
        assertTrue(of("{\"target\":\"prometheus\"}").isTargetExplicitlySet());
        assertEquals("prometheus", of("{\"target\":\"prometheus\"}").getTarget());
    }

    @Test
    void omittedTargetLeavesFlagUnsetAndDefaultsToLog() {
        MetricConfiguration cfg = of("{\"namespace\":\"ns1\"}");
        assertFalse(cfg.isTargetExplicitlySet());
        assertEquals("log", cfg.getTarget());
    }

    @Test
    void prometheusPortAndPathParsed() {
        MetricConfiguration cfg = of(
                "{\"target\":\"prometheus\",\"targetConfig\":{\"port\":9111,\"path\":\"/m\"}}");
        assertEquals(9111, cfg.getPrometheusPort());
        assertEquals("/m", cfg.getPrometheusPath());
    }

    @Test
    void prometheusPortPathDefaultsWhenTargetConfigOmitsThem() {
        MetricConfiguration cfg = of("{\"target\":\"prometheus\"}");
        assertEquals(9090, cfg.getPrometheusPort());
        assertEquals("/metrics", cfg.getPrometheusPath());
    }

    @Test
    void portPathReadEvenWhenTargetDefaulted() {
        // KUBERNETES default case: target omitted (defaults to "log" here) but an explicit port/path
        // is still parsed so the profile-selected prometheus target honors it.
        MetricConfiguration cfg = of("{\"targetConfig\":{\"port\":7000,\"path\":\"/p\"}}");
        assertFalse(cfg.isTargetExplicitlySet());
        assertEquals(7000, cfg.getPrometheusPort());
        assertEquals("/p", cfg.getPrometheusPath());
    }
}
