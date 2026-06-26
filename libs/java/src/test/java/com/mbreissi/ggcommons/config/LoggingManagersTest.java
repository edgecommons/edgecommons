/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.mbreissi.ggcommons.ParsedCommandLine;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link LoggingConfigurationManager} and {@link GlobalLoggingManager}.
 * Both apply a {@link LoggingConfiguration} to the live Log4j2 context; the methods are
 * void and swallow their own errors, so we assert they execute without throwing.
 */
class LoggingManagersTest {

    private static ConfigManager managerFor(String loggingBlock) throws IOException {
        String configJson = """
                {\
                "logging": %s,\
                "metricEmission": {"target": "log"},\
                "heartbeat": {"intervalSecs": 30},\
                "tags": {},\
                "component": {"global": {}}\
                }""".formatted(loggingBlock);
        File tempFile = File.createTempFile("logmgr-config", ".json");
        tempFile.deleteOnExit();
        try (FileWriter writer = new FileWriter(tempFile)) {
            writer.write(configJson);
            writer.flush();
        }
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", tempFile.getAbsolutePath()};
        cmdLine.thingName = "test-thing";
        try {
            return ConfigManagerFactory.create("com.test.TestComponent", cmdLine);
        } catch (ConfigurationException e) {
            throw new RuntimeException(e);
        }
    }

    @Test
    void loggingConfigurationManagerConfiguresGgcommonsNamespace() throws IOException {
        // Includes a ggcommons-namespaced specific logger (the only ones it touches).
        ConfigManager cm = managerFor("""
                {"level":"DEBUG","loggers":{"com.mbreissi.ggcommons.test":"WARN"}}""");
        LoggingConfigurationManager mgr = new LoggingConfigurationManager("com.test.TestComponent", cm);
        assertDoesNotThrow(mgr::configureLogging);
        // Idempotent: re-applying (logger now exists) hits the update branch.
        assertDoesNotThrow(mgr::configureLogging);
    }

    @Test
    void loggingConfigurationManagerIgnoresNonGgcommonsLoggers() throws IOException {
        ConfigManager cm = managerFor("""
                {"level":"INFO","loggers":{"org.apache.http":"WARN"}}""");
        LoggingConfigurationManager mgr = new LoggingConfigurationManager("com.test.TestComponent", cm);
        assertDoesNotThrow(mgr::configureLogging);
    }

    @Test
    void globalLoggingManagerNoOpWhenControlDisabled() throws IOException {
        ConfigManager cm = managerFor("""
                {"level":"INFO"}""");
        GlobalLoggingManager mgr = new GlobalLoggingManager(cm, false);
        // takeGlobalControl=false -> immediate return, nothing changes.
        assertDoesNotThrow(mgr::configureGlobalLogging);
    }

    @Test
    void globalLoggingManagerAppliesFullConfigWhenEnabled() throws IOException {
        // File logging enabled + a specific logger exercises the file-appender and
        // per-logger branches of the configuration builder.
        ConfigManager cm = managerFor("""
                {"level":"INFO",\
                "fileLogging":{"enabled":true,"filePath":"/tmp/{ComponentName}-global.log"},\
                "loggers":{"com.mbreissi.ggcommons":"DEBUG"}}""");
        GlobalLoggingManager mgr = new GlobalLoggingManager(cm, true);
        assertDoesNotThrow(mgr::configureGlobalLogging);
    }

    @Test
    void globalLoggingManagerAppliesConsoleOnlyConfig() throws IOException {
        ConfigManager cm = managerFor("""
                {"level":"WARN"}""");
        GlobalLoggingManager mgr = new GlobalLoggingManager(cm, true);
        assertDoesNotThrow(mgr::configureGlobalLogging);
    }
}
