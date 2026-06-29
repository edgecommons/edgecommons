/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.metrics.targets;

import com.mbreissi.ggcommons.config.ConfigurationFactory;
import com.mbreissi.ggcommons.config.MetricConfiguration;
import com.mbreissi.ggcommons.metrics.Metric;
import com.mbreissi.ggcommons.metrics.MetricBuilder;
import com.mbreissi.ggcommons.platform.Platform;
import com.mbreissi.ggcommons.platform.PlatformResolver;
import com.mbreissi.ggcommons.test.MockConfigurationService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.nio.file.Path;
import java.util.HashMap;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Tests the HOST-aware default metric-log path (Part B): the metric {@code log} target resolves its
 * file path with the precedence explicit {@code logFileName} config ▸ the platform-profile default
 * (a local path on HOST/KUBERNETES) ▸ the library default ({@code /greengrass/v2/logs}).
 */
class MetricLogPlatformPathTest {

    // ----- resolver: profileMetricLogPath -----------------------------------------------------

    @Test
    void profileMetricLogPathIsLocalForHostAndKubernetes() {
        assertEquals(PlatformResolver.METRIC_LOG_PATH_LOCAL,
                PlatformResolver.profileMetricLogPath(Platform.HOST));
        assertEquals(PlatformResolver.METRIC_LOG_PATH_LOCAL,
                PlatformResolver.profileMetricLogPath(Platform.KUBERNETES));
    }

    @Test
    void profileMetricLogPathIsNullForGreengrassAndNull() {
        assertNull(PlatformResolver.profileMetricLogPath(Platform.GREENGRASS));
        assertNull(PlatformResolver.profileMetricLogPath(null));
    }

    // ----- config: getExplicitLogFileName -----------------------------------------------------

    @Test
    void explicitLogFileNameAbsentWhenNotConfigured() {
        assertNull(metricConfig("{\"target\":\"log\"}").getExplicitLogFileName());
    }

    @Test
    void explicitLogFileNamePresentWhenConfigured() {
        MetricConfiguration mc =
                metricConfig("{\"target\":\"log\",\"targetConfig\":{\"logFileName\":\"/custom/x.log\"}}");
        assertEquals("/custom/x.log", mc.getExplicitLogFileName());
        assertEquals("/custom/x.log", mc.getLogFileNameTemplate());
    }

    // ----- target: path precedence ------------------------------------------------------------

    @Test
    void hostWithNoExplicitFileUsesLocalDefaultTemplate(@TempDir Path tempDir) {
        CapturingCM cm = new CapturingCM("{\"target\":\"log\"}", tempDir);
        Log log = new Log(cm);
        assertDoesNotThrow(() -> log.emitMetricNow(metric(), values()));
        log.close();

        assertEquals(PlatformResolver.METRIC_LOG_PATH_LOCAL, cm.captured,
                "HOST + no explicit logFileName should resolve the platform-default local template");
        assertTrue(tempDir.resolve("metric.log").toFile().exists());
    }

    @Test
    void explicitFileWinsOverPlatformDefault(@TempDir Path tempDir) {
        CapturingCM cm = new CapturingCM(
                "{\"target\":\"log\",\"targetConfig\":{\"logFileName\":\"/explicit/here.log\"}}", tempDir);
        Log log = new Log(cm);
        assertDoesNotThrow(() -> log.emitMetricNow(metric(), values()));
        log.close();

        assertEquals("/explicit/here.log", cm.captured,
                "an explicit logFileName must win over the platform default");
    }

    // ----- helpers ----------------------------------------------------------------------------

    private static MetricConfiguration metricConfig(String json) {
        JsonObject root = new JsonObject();
        root.add("metricEmission", JsonParser.parseString(json).getAsJsonObject());
        return ConfigurationFactory.createMetricConfiguration(root);
    }

    private static Metric metric() {
        return MetricBuilder.create("m1").withNamespace("ns1").addMeasure("value", "Count", 60).build();
    }

    private static HashMap<String, Float> values() {
        HashMap<String, Float> v = new HashMap<>();
        v.put("value", 1.0f);
        return v;
    }

    /**
     * A HOST config manager that records the template the {@link Log} target asks to resolve, and
     * redirects whatever it resolves to a temp file (so a relative {@code ./logs/...} template never
     * pollutes the build's working directory).
     */
    private static final class CapturingCM extends MockConfigurationService {
        private final MetricConfiguration metricConfig;
        private final Path dir;
        String captured;

        CapturingCM(String metricJson, Path dir) {
            this.metricConfig = metricConfig(metricJson);
            this.dir = dir;
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }

        @Override
        public Platform getPlatform() {
            return Platform.HOST;
        }

        @Override
        public String resolveTemplate(String template) {
            this.captured = template;
            return dir.resolve("metric.log").toString();
        }
    }
}
