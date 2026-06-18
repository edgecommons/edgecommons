/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for the config model classes constructed directly from a {@link JsonObject}.
 * Exercises getters and {@code toDict()} for {@link LoggingConfiguration},
 * {@link MetricConfiguration}, {@link HeartbeatConfiguration} and {@link TagConfiguration}
 * across the various target/measure combinations, plus the {@link ConfigurationFactory}
 * create* methods. Distinct from {@code ConfigManagerTest} (which drives the same classes
 * through the full ConfigManager) and {@code ConfigSerializationTest} (regression-only).
 */
class ConfigModelTest {

    private static JsonObject obj(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    // ---------------------------------------------------------------------
    // LoggingConfiguration
    // ---------------------------------------------------------------------

    @Test
    void loggingConfigurationDefaultsWhenNullJson() {
        LoggingConfiguration cfg = new LoggingConfiguration(null);
        assertEquals("INFO", cfg.getLevel().toString());
        assertEquals(LoggingConfiguration.DEFAULT_FORMAT, cfg.getFormat());
        assertFalse(cfg.isFileLoggingEnabled());
        assertNull(cfg.getLogFilePath());
        assertTrue(cfg.getLoggerLevels().isEmpty());
        assertFalse(cfg.isGlobalControlEnabled());

        // toDict on defaults: only level + format, no optional blocks.
        JsonObject dict = cfg.toDict();
        assertEquals("INFO", dict.get("level").getAsString());
        assertEquals(LoggingConfiguration.DEFAULT_FORMAT, dict.get("format").getAsString());
        assertFalse(dict.has("fileLogging"));
        assertFalse(dict.has("loggers"));
        assertFalse(dict.has("globalControl"));
    }

    @Test
    void loggingConfigurationFullyPopulatedAndToString() {
        LoggingConfiguration cfg = new LoggingConfiguration(obj("""
                {"level":"DEBUG","format":"%m%n",\
                "fileLogging":{"enabled":true,"filePath":"/tmp/app.log"},\
                "loggers":{"com.aws.proserve":"warn"},\
                "globalControl":true}"""));

        assertEquals("DEBUG", cfg.getLevel().toString());
        assertEquals("%m%n", cfg.getFormat());
        assertTrue(cfg.isFileLoggingEnabled());
        assertEquals("/tmp/app.log", cfg.getLogFilePath());
        assertTrue(cfg.isGlobalControlEnabled());
        assertEquals(1, cfg.getLoggerLevels().size());
        assertEquals("WARN", cfg.getLoggerLevels().get("com.aws.proserve").toString());

        JsonObject dict = cfg.toDict();
        assertEquals("DEBUG", dict.get("level").getAsString());
        assertTrue(dict.getAsJsonObject("fileLogging").get("enabled").getAsBoolean());
        assertEquals("/tmp/app.log", dict.getAsJsonObject("fileLogging").get("filePath").getAsString());
        assertEquals("WARN", dict.getAsJsonObject("loggers").get("com.aws.proserve").getAsString());
        assertTrue(dict.get("globalControl").getAsBoolean());

        // toString() must produce parseable JSON equal to toDict().
        assertEquals(dict, obj(cfg.toString()));
    }

    @Test
    void loggingConfigurationFileLoggingEnabledWithoutPath() {
        LoggingConfiguration cfg = new LoggingConfiguration(obj("""
                {"fileLogging":{"enabled":true}}"""));
        assertTrue(cfg.isFileLoggingEnabled());
        assertNull(cfg.getLogFilePath());
        JsonObject fileDict = cfg.toDict().getAsJsonObject("fileLogging");
        assertTrue(fileDict.get("enabled").getAsBoolean());
        assertFalse(fileDict.has("filePath"));
    }

    @Test
    void loggingConfigurationGetLoggerLevelsIsUnmodifiable() {
        LoggingConfiguration cfg = new LoggingConfiguration(obj("""
                {"level":"INFO"}"""));
        assertThrows(UnsupportedOperationException.class,
                () -> cfg.getLoggerLevels().put("x", org.apache.logging.log4j.Level.INFO));
    }

    // ---------------------------------------------------------------------
    // MetricConfiguration (each target branch)
    // ---------------------------------------------------------------------

    @Test
    void metricConfigurationDefaultsWhenNullJson() {
        MetricConfiguration cfg = new MetricConfiguration(null);
        assertEquals("log", cfg.getTarget());
        assertEquals("ggcommons", cfg.getNamespace());
        assertEquals(5, cfg.getIntervalSecs());
        assertEquals("ipc", cfg.getDestination());
        assertFalse(cfg.getLargeFleetWorkaround());
        assertEquals("10MB", cfg.getMaxFileSize());
        assertNull(cfg.getTopic());
    }

    @Test
    void metricConfigurationLogTarget() {
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"log","namespace":"ns",\
                "targetConfig":{"logFileName":"/var/m.log","maxFileSize":"50MB"}}"""));
        assertEquals("log", cfg.getTarget());
        assertEquals("ns", cfg.getNamespace());
        assertEquals("/var/m.log", cfg.getLogFileNameTemplate());
        assertEquals("50MB", cfg.getMaxFileSize());

        JsonObject tc = cfg.toDict().getAsJsonObject("targetConfig");
        assertEquals("/var/m.log", tc.get("filename").getAsString());
        assertEquals("50MB", tc.get("maxFileSize").getAsString());
        assertEquals(cfg.toDict().toString(), cfg.toString());
    }

    @Test
    void metricConfigurationMessagingTarget() {
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"messaging",\
                "targetConfig":{"topic":"a/b/c","destination":"iotcore"}}"""));
        assertEquals("messaging", cfg.getTarget());
        assertEquals("a/b/c", cfg.getTopic());
        assertEquals("iotcore", cfg.getDestination());

        JsonObject tc = cfg.toDict().getAsJsonObject("targetConfig");
        assertEquals("a/b/c", tc.get("topic").getAsString());
        assertEquals("iotcore", tc.get("destination").getAsString());
    }

    @Test
    void metricConfigurationMessagingTargetUsesDefaultTopic() {
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"messaging"}"""));
        assertEquals("{ThingName}/{ComponentName}/metric", cfg.getTopic());
        assertEquals("ipc", cfg.getDestination());
    }

    @Test
    void metricConfigurationCloudwatchTarget() {
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"cloudwatch","largeFleetWorkaround":true,\
                "targetConfig":{"intervalSecs":42}}"""));
        assertEquals("cloudwatch", cfg.getTarget());
        assertEquals(42, cfg.getIntervalSecs());
        assertTrue(cfg.getLargeFleetWorkaround());
        assertEquals(42, cfg.toDict().getAsJsonObject("targetConfig").get("intervalSecs").getAsInt());
    }

    @Test
    void metricConfigurationCloudwatchIntervalBelowOneResetsToDefault() {
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"cloudwatch","targetConfig":{"intervalSecs":0}}"""));
        assertEquals(5, cfg.getIntervalSecs());
    }

    @Test
    void metricConfigurationCloudwatchComponentTarget() {
        // Default topic for cloudwatchcomponent when no targetConfig.topic supplied.
        MetricConfiguration deflt = new MetricConfiguration(obj("""
                {"target":"cloudwatchcomponent"}"""));
        assertEquals("cloudwatchcomponent", deflt.getTarget());
        assertEquals("cloudwatch/metric/put", deflt.getTopic());

        // Overridden topic.
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"cloudwatchcomponent","targetConfig":{"topic":"cw/custom"}}"""));
        assertEquals("cw/custom", cfg.getTopic());

        // toDict() has no switch case for cloudwatchcomponent -> empty targetConfig block.
        JsonObject dict = cfg.toDict();
        assertEquals("cloudwatchcomponent", dict.get("target").getAsString());
        assertEquals(0, dict.getAsJsonObject("targetConfig").size());
    }

    // ---------------------------------------------------------------------
    // HeartbeatConfiguration
    // ---------------------------------------------------------------------

    @Test
    void heartbeatConfigurationDefaultsWhenNullJson() {
        HeartbeatConfiguration cfg = new HeartbeatConfiguration(null);
        assertEquals(5, cfg.getIntervalSecs());
        assertTrue(cfg.includeCpu());
        assertTrue(cfg.includeMemory());
        assertFalse(cfg.includeDisk());
        assertFalse(cfg.includeThreads());
        assertFalse(cfg.includeFiles());
        assertFalse(cfg.includeFds());
        // No targets supplied -> default single "metric" target is injected.
        assertEquals(1, cfg.getTargets().size());
        assertEquals("metric", cfg.getTargets().get(0).getType());
        assertNull(cfg.getTargets().get(0).getConfig());
    }

    @Test
    void heartbeatConfigurationIntervalBelowOneResetsToDefault() {
        HeartbeatConfiguration cfg = new HeartbeatConfiguration(obj("""
                {"intervalSecs":0}"""));
        assertEquals(5, cfg.getIntervalSecs());
    }

    @Test
    void heartbeatConfigurationMeasuresAndTargets() {
        HeartbeatConfiguration cfg = new HeartbeatConfiguration(obj("""
                {"intervalSecs":15,\
                "measures":{"cpu":false,"memory":true,"disk":true,"threads":true,"files":true,"fds":true},\
                "targets":[\
                {"type":"metric"},\
                {"type":"messaging","config":{"destination":"ipc","topic":"hb/t"}},\
                {"type":"bogus"}]}"""));

        assertEquals(15, cfg.getIntervalSecs());
        assertFalse(cfg.includeCpu());
        assertTrue(cfg.includeMemory());
        // disk/fds are warned & ignored in java -> remain default false.
        assertFalse(cfg.includeDisk());
        assertFalse(cfg.includeFds());
        assertTrue(cfg.includeThreads());
        assertTrue(cfg.includeFiles());

        // bogus target dropped; metric + messaging retained.
        List<HeartbeatConfiguration.HeartbeatTarget> targets = cfg.getTargets();
        assertEquals(2, targets.size());
        assertEquals("metric", targets.get(0).getType());
        assertEquals("messaging", targets.get(1).getType());
        assertNotNull(targets.get(1).getConfig());
        assertEquals("hb/t", targets.get(1).getConfig().get("topic").getAsString());

        // toDict reflects measures + targets and toString mirrors it.
        JsonObject dict = cfg.toDict();
        assertEquals(15, dict.get("intervalSecs").getAsInt());
        assertEquals(2, dict.getAsJsonArray("targets").size());
        assertEquals(dict.toString(), cfg.toString());
    }

    // ---------------------------------------------------------------------
    // TagConfiguration
    // ---------------------------------------------------------------------

    @Test
    void tagConfigurationNullJsonYieldsEmptyTags() {
        TagConfiguration cfg = new TagConfiguration(null);
        assertTrue(cfg.getKeys().isEmpty());
        assertEquals(0, cfg.toDict().size());
        assertEquals("{}", cfg.toString());
    }

    @Test
    void tagConfigurationKeysValuesAndRoundTrip() {
        JsonObject tags = obj("""
                {"env":"prod","region":"us-east-1"}""");
        TagConfiguration cfg = new TagConfiguration(tags);
        assertTrue(cfg.getKeys().contains("env"));
        assertTrue(cfg.getKeys().contains("region"));
        assertEquals("prod", cfg.getKeyValue("env"));
        assertEquals("us-east-1", cfg.getKeyValue("region"));

        // toDict returns the backing object; round-trip through constructor preserves it.
        assertEquals(tags, cfg.toDict());
        TagConfiguration roundTrip = new TagConfiguration(cfg.toDict());
        assertEquals("prod", roundTrip.getKeyValue("env"));
    }

    // ---------------------------------------------------------------------
    // ConfigurationFactory
    // ---------------------------------------------------------------------

    @Test
    void factoryReturnsDefaultsForNullConfig() {
        assertEquals("INFO", ConfigurationFactory.createLoggingConfiguration(null).getLevel().toString());
        assertEquals("log", ConfigurationFactory.createMetricConfiguration(null).getTarget());
        assertEquals(5, ConfigurationFactory.createHeartbeatConfiguration(null).getIntervalSecs());
        assertTrue(ConfigurationFactory.createTagConfiguration(null).getKeys().isEmpty());
    }

    @Test
    void factoryReturnsDefaultsWhenSectionsAbsent() {
        JsonObject empty = obj("""
                {}""");
        assertEquals("INFO", ConfigurationFactory.createLoggingConfiguration(empty).getLevel().toString());
        assertEquals("log", ConfigurationFactory.createMetricConfiguration(empty).getTarget());
        assertEquals(5, ConfigurationFactory.createHeartbeatConfiguration(empty).getIntervalSecs());
        assertTrue(ConfigurationFactory.createTagConfiguration(empty).getKeys().isEmpty());
    }

    @Test
    void factoryParsesPopulatedSections() {
        JsonObject cfg = obj("""
                {"logging":{"level":"DEBUG"},\
                "metricEmission":{"target":"cloudwatch"},\
                "heartbeat":{"intervalSecs":20},\
                "tags":{"k":"v"}}""");

        assertEquals("DEBUG", ConfigurationFactory.createLoggingConfiguration(cfg).getLevel().toString());
        assertEquals("cloudwatch", ConfigurationFactory.createMetricConfiguration(cfg).getTarget());
        assertEquals(20, ConfigurationFactory.createHeartbeatConfiguration(cfg).getIntervalSecs());
        assertEquals("v", ConfigurationFactory.createTagConfiguration(cfg).getKeyValue("k"));
    }
}
