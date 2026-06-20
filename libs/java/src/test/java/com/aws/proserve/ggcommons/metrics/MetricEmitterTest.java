/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.metrics;

import com.aws.proserve.ggcommons.config.MetricConfiguration;
import com.aws.proserve.ggcommons.config.ConfigurationFactory;
import com.aws.proserve.ggcommons.test.MockConfigurationService;
import com.aws.proserve.ggcommons.test.MockMessagingService;
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
 * Unit tests for {@link MetricEmitter}. Exercises target selection (messaging / log / invalid),
 * metric definition gating, and the flush/close delegation to the underlying target.
 *
 * <p>The CloudWatch target is intentionally not constructed here as its real constructor builds a
 * live {@link software.amazon.awssdk.services.cloudwatch.CloudWatchClient}; CloudWatch behavior is
 * covered directly by CloudWatchTest with a mocked client.
 */
class MetricEmitterTest {

    /** Config returning a caller-supplied metric configuration. */
    private static class EmitterConfig extends MockConfigurationService {
        private final MetricConfiguration metricConfig;

        EmitterConfig(String metricJson) {
            var root = new JsonObject();
            root.add("metricEmission", JsonParser.parseString(metricJson).getAsJsonObject());
            this.metricConfig = ConfigurationFactory.createMetricConfiguration(root);
        }

        @Override
        public MetricConfiguration getMetricConfig() {
            return metricConfig;
        }
    }

    private static Metric metric(String name) {
        return MetricBuilder.create(name)
                .withNamespace("ns1")
                .addMeasure("value", "Count", 60)
                .build();
    }

    private static Map<String, Float> values() {
        var v = new HashMap<String, Float>();
        v.put("value", 1.0f);
        return v;
    }

    @Test
    void noArgConstructorYieldsNullConfigAndSafeFlushClose() {
        // Protected constructor is reachable from the same package via an anonymous subclass.
        var emitter = new MetricEmitter() {};
        assertNull(emitter.getMetricConfig());
        assertNull(emitter.getThingName());
        assertNull(emitter.getComponentName());
        // flush/close guard against a null target.
        assertDoesNotThrow(emitter::flushMetrics);
        assertDoesNotThrow(emitter::close);
    }

    @Test
    void messagingTargetEmitsThroughInjectedService() {
        var config = new EmitterConfig("""
                {"target":"messaging","namespace":"ns1","targetConfig":{"topic":"t/topic","destination":"ipc"}}""");
        var mock = new MockMessagingService();
        var emitter = new MetricEmitter(config, mock);

        assertEquals("test-thing", emitter.getThingName());
        assertEquals("TestComponent", emitter.getComponentName());
        assertNotNull(emitter.getMetricConfig());

        emitter.defineMetric(metric("m1"));
        assertTrue(emitter.isMetricDefined("m1"));
        assertFalse(emitter.isMetricDefined("nope"));

        emitter.emitMetric("m1", values());
        emitter.emitMetricNow("m1", values());
        assertEquals(2, mock.getPublishedMessages().size());

        emitter.close();
    }

    @Test
    void emitUndefinedMetricIsIgnored() {
        var config = new EmitterConfig("""
                {"target":"messaging","namespace":"ns1","targetConfig":{"topic":"t/topic","destination":"ipc"}}""");
        var mock = new MockMessagingService();
        var emitter = new MetricEmitter(config, mock);

        emitter.emitMetric("undefined", values());
        emitter.emitMetricNow("undefined", values());
        assertTrue(mock.getPublishedMessages().isEmpty());

        emitter.close();
    }

    @Test
    void logTargetIsSelectedAndFlushClose(@TempDir Path tempDir) {
        File logFile = tempDir.resolve("emitter-metrics.log").toFile();
        String json = "{\"target\":\"log\",\"namespace\":\"ns1\",\"targetConfig\":{\"logFileName\":\""
                + logFile.getAbsolutePath().replace("\\", "\\\\") + "\",\"maxFileSize\":\"10MB\"}}";
        var config = new EmitterConfig(json);
        var emitter = new MetricEmitter(config, null);

        emitter.defineMetric(metric("m1"));
        emitter.emitMetricNow("m1", values());
        assertDoesNotThrow(emitter::flushMetrics);
        emitter.close();

        assertTrue(logFile.exists());
    }

    @Test
    void invalidTargetDefaultsToLog() {
        // An unknown target should fall back to the log target. The default log path may not be
        // writable in the test environment, but Log internally falls back to a logger without
        // throwing, so emit must complete cleanly. (logFileName under a non-"log" target is
        // ignored by MetricConfiguration, so the emitter uses the default Greengrass path here.)
        var config = new EmitterConfig("""
                {"target":"bogus","namespace":"ns1"}""");
        var emitter = new MetricEmitter(config, null);

        emitter.defineMetric(metric("m1"));
        assertDoesNotThrow(() -> emitter.emitMetricNow("m1", values()));
        emitter.close();
    }

    @Test
    void cloudwatchComponentTargetUsesInjectedMessaging() {
        var config = new EmitterConfig("""
                {"target":"cloudwatchcomponent","namespace":"ns1","targetConfig":{"topic":"cw/put"}}""");
        var mock = new MockMessagingService();
        var emitter = new MetricEmitter(config, mock);

        emitter.defineMetric(metric("m1"));
        emitter.emitMetricNow("m1", values());
        assertEquals(1, mock.getPublishedMessages().size());
        assertEquals("cw/put", mock.getPublishedMessages().get(0).topic);

        emitter.close();
    }
}
