/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

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

        // toDict on defaults: only level + java_format, no optional blocks.
        JsonObject dict = cfg.toDict();
        assertEquals("INFO", dict.get("level").getAsString());
        assertEquals(LoggingConfiguration.DEFAULT_FORMAT, dict.get("java_format").getAsString());
        assertFalse(dict.has("fileLogging"));
        assertFalse(dict.has("loggers"));
        assertFalse(dict.has("globalControl"));
    }

    @Test
    void loggingConfigurationFullyPopulatedAndToString() {
        LoggingConfiguration cfg = new LoggingConfiguration(obj("""
                {"level":"DEBUG","java_format":"%m%n",\
                "fileLogging":{"enabled":true,"filePath":"/tmp/app.log"},\
                "loggers":{"com.mbreissi":"warn"},\
                "globalControl":true}"""));

        assertEquals("DEBUG", cfg.getLevel().toString());
        assertEquals("%m%n", cfg.getFormat());
        assertTrue(cfg.isFileLoggingEnabled());
        assertEquals("/tmp/app.log", cfg.getLogFilePath());
        assertTrue(cfg.isGlobalControlEnabled());
        assertEquals(1, cfg.getLoggerLevels().size());
        assertEquals("WARN", cfg.getLoggerLevels().get("com.mbreissi").toString());

        JsonObject dict = cfg.toDict();
        assertEquals("DEBUG", dict.get("level").getAsString());
        assertTrue(dict.getAsJsonObject("fileLogging").get("enabled").getAsBoolean());
        assertEquals("/tmp/app.log", dict.getAsJsonObject("fileLogging").get("filePath").getAsString());
        assertEquals("WARN", dict.getAsJsonObject("loggers").get("com.mbreissi").getAsString());
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
        // §4.3/D-U9: targetConfig.topic is removed — only the destination survives; the
        // Messaging target builds the UNS metric topic itself.
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"messaging",\
                "targetConfig":{"destination":"iotcore"}}"""));
        assertEquals("messaging", cfg.getTarget());
        assertNull(cfg.getTopic(), "the messaging target no longer carries a configured topic");
        assertEquals("iotcore", cfg.getDestination());

        JsonObject tc = cfg.toDict().getAsJsonObject("targetConfig");
        assertFalse(tc.has("topic"), "toDict must not emit a topic for the messaging target");
        assertEquals("iotcore", tc.get("destination").getAsString());
    }

    @Test
    void metricConfigurationMessagingTargetDefaults() {
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"messaging"}"""));
        assertNull(cfg.getTopic());
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
        // The cloudwatchcomponent topic is the fixed external Greengrass contract (D-U21);
        // the former targetConfig.topic override is removed with the schema key.
        MetricConfiguration cfg = new MetricConfiguration(obj("""
                {"target":"cloudwatchcomponent"}"""));
        assertEquals("cloudwatchcomponent", cfg.getTarget());
        assertEquals("cloudwatch/metric/put", cfg.getTopic());

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
        // D-U14/M11: the heartbeat defaults are on / 5 s / local (the injected legacy
        // "metric" default target is gone with targets[]).
        HeartbeatConfiguration cfg = new HeartbeatConfiguration(null);
        assertTrue(cfg.isEnabled());
        assertEquals(5, cfg.getIntervalSecs());
        assertEquals("local", cfg.getDestination());
        assertTrue(cfg.includeCpu());
        assertTrue(cfg.includeMemory());
        assertFalse(cfg.includeDisk());
        assertFalse(cfg.includeThreads());
        assertFalse(cfg.includeFiles());
        assertFalse(cfg.includeFds());
    }

    @Test
    void heartbeatConfigurationIntervalBelowOneResetsToDefault() {
        HeartbeatConfiguration cfg = new HeartbeatConfiguration(obj("""
                {"intervalSecs":0}"""));
        assertEquals(5, cfg.getIntervalSecs());
    }

    @Test
    void heartbeatConfigurationMeasuresEnabledAndDestination() {
        HeartbeatConfiguration cfg = new HeartbeatConfiguration(obj("""
                {"enabled":false,"intervalSecs":15,\
                "measures":{"cpu":false,"memory":true,"disk":true,"threads":true,"files":true,"fds":true},\
                "destination":"iotcore"}"""));

        assertFalse(cfg.isEnabled());
        assertEquals(15, cfg.getIntervalSecs());
        assertEquals("iotcore", cfg.getDestination());
        assertFalse(cfg.includeCpu());
        assertTrue(cfg.includeMemory());
        // disk/fds are now honored in Java (collected via File + Unix OS MXBean).
        assertTrue(cfg.includeDisk());
        assertTrue(cfg.includeFds());
        assertTrue(cfg.includeThreads());
        assertTrue(cfg.includeFiles());

        // toDict reflects enabled + measures + destination and toString mirrors it.
        JsonObject dict = cfg.toDict();
        assertFalse(dict.get("enabled").getAsBoolean());
        assertEquals(15, dict.get("intervalSecs").getAsInt());
        assertEquals("iotcore", dict.get("destination").getAsString());
        assertFalse(dict.has("targets"), "the legacy targets[] array is removed (D-U20)");
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
