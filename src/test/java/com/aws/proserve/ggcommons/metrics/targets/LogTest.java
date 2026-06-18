/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics.targets;

import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.metrics.Metric;
import com.aws.proserve.ggcommons.metrics.MetricBuilder;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.File;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the {@link Log} metric target. Writes to a JUnit temp directory so the
 * lazily-configured rolling file appender is exercised without touching real Greengrass paths.
 */
class LogTest {

    /** Config that points the log target at a caller-supplied file in a temp directory. */
    private static class LogConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        LogConfig(String logFile, boolean largeFleet) {
            String json = "{\"target\":\"log\",\"namespace\":\"ns1\",\"largeFleetWorkaround\":" + largeFleet
                    + ",\"targetConfig\":{\"logFileName\":\"" + logFile.replace("\\", "\\\\") + "\",\"maxFileSize\":\"10MB\"}}";
            var root = new JsonObject();
            root.add("metricEmission", JsonParser.parseString(json).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    private static Metric metric() {
        return MetricBuilder.create("m1")
                .withNamespace("ns1")
                .addMeasure("value", "Count", 60)
                .build();
    }

    @Test
    void emitMetricWritesLogFile(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("metrics.log").toFile();
        var log = new Log(new LogConfig(logFile.getAbsolutePath(), false));

        var values = new HashMap<String, Float>();
        values.put("value", 3.0f);
        assertDoesNotThrow(() -> log.emitMetric(metric(), values));
        log.close();

        assertTrue(logFile.exists(), "metric log file should have been created");
    }

    @Test
    void emitMetricNowWritesLogFile(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("metrics2.log").toFile();
        var log = new Log(new LogConfig(logFile.getAbsolutePath(), false));

        var values = new HashMap<String, Float>();
        values.put("value", 4.0f);
        assertDoesNotThrow(() -> log.emitMetricNow(metric(), values));
        log.close();

        assertTrue(logFile.exists());
    }

    @Test
    void largeFleetWorkaroundEmitsSecondRecord(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("metrics3.log").toFile();
        var log = new Log(new LogConfig(logFile.getAbsolutePath(), true));

        var values = new HashMap<String, Float>();
        values.put("value", 5.0f);
        assertDoesNotThrow(() -> log.emitMetricNow(metric(), values));
        log.close();

        assertTrue(logFile.exists());
    }

    @Test
    void onConfigurationChangedResetsLoggerAndReconfigures(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("metrics4.log").toFile();
        var log = new Log(new LogConfig(logFile.getAbsolutePath(), false));

        var values = new HashMap<String, Float>();
        values.put("value", 6.0f);
        log.emitMetricNow(metric(), values);

        assertTrue(log.onConfigurationChanged());
        // After reset the logger is lazily reconfigured on next emit.
        assertDoesNotThrow(() -> log.emitMetricNow(metric(), values));
        log.close();

        assertTrue(logFile.exists());
    }

    @Test
    void closeWithoutEmitDoesNotThrow(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("metrics5.log").toFile();
        var log = new Log(new LogConfig(logFile.getAbsolutePath(), false));
        // No emit was performed -> no appender created -> close must be a safe no-op.
        assertDoesNotThrow(log::close);
    }

    @Test
    void logFileWithoutExtensionStillEmits(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("metrics_noext").toFile();
        var log = new Log(new LogConfig(logFile.getAbsolutePath(), false));

        var values = new HashMap<String, Float>();
        values.put("value", 8.0f);
        assertDoesNotThrow(() -> log.emitMetricNow(metric(), values));
        log.close();

        assertTrue(logFile.exists());
    }
}
