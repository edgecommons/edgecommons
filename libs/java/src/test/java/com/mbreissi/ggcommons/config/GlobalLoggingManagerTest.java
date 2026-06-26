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

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;

/**
 * Unit tests for {@link GlobalLoggingManager}.
 *
 * <p>The manager rebuilds the Log4j2 configuration in-process. These tests drive the
 * public {@link GlobalLoggingManager#configureGlobalLogging()} path under several
 * configurations (control disabled, console-only, file logging enabled, and explicit
 * per-logger levels) to maximise line coverage. There is no externally observable
 * return value, so the tests assert the calls do not throw.
 */
class GlobalLoggingManagerTest {

    /**
     * With {@code takeGlobalControl == false} the manager must return immediately
     * without touching the logging system.
     */
    @Test
    void noControlIsNoOp() throws IOException {
        ConfigManager cm = configManagerFor(loggingConfigJson(false, false, false));
        GlobalLoggingManager manager = new GlobalLoggingManager(cm, false);
        assertDoesNotThrow(manager::configureGlobalLogging);
    }

    /**
     * Console-only configuration with global control enabled.
     */
    @Test
    void consoleOnlyConfiguration() throws IOException {
        ConfigManager cm = configManagerFor(loggingConfigJson(true, false, false));
        GlobalLoggingManager manager = new GlobalLoggingManager(cm, true);
        assertDoesNotThrow(manager::configureGlobalLogging);
    }

    /**
     * File logging enabled drives the file-appender and template-resolution branches.
     */
    @Test
    void fileLoggingConfiguration() throws IOException {
        File logDir = File.createTempFile("ggcommons-log", "");
        logDir.delete();
        logDir.mkdirs();
        logDir.deleteOnExit();
        String logFilePath = new File(logDir, "app.log").getAbsolutePath().replace("\\", "/");

        ConfigManager cm = configManagerFor(loggingConfigJsonWithFile(logFilePath));
        GlobalLoggingManager manager = new GlobalLoggingManager(cm, true);
        assertDoesNotThrow(manager::configureGlobalLogging);
    }

    /**
     * Explicit per-logger levels drive the specific-logger loop.
     */
    @Test
    void perLoggerLevelsConfiguration() throws IOException {
        ConfigManager cm = configManagerFor(loggingConfigJson(true, false, true));
        GlobalLoggingManager manager = new GlobalLoggingManager(cm, true);
        assertDoesNotThrow(manager::configureGlobalLogging);
    }

    // --- helpers -----------------------------------------------------------

    private static String loggingConfigJson(boolean globalControl, boolean fileLogging, boolean withLoggers) {
        StringBuilder logging = new StringBuilder();
        logging.append("\"level\": \"INFO\",");
        logging.append("\"java_format\": \"%d [%level] %logger - %msg%n\",");
        logging.append("\"fileLogging\": {\"enabled\": ").append(fileLogging).append("},");
        if (withLoggers) {
            logging.append("\"loggers\": {\"com.mbreissi\": \"DEBUG\", \"org.apache\": \"WARN\"},");
        }
        logging.append("\"globalControl\": ").append(globalControl);

        return "{" +
                "\"logging\": {" + logging + "}," +
                "\"metricEmission\": {\"target\": \"log\"}," +
                "\"heartbeat\": {\"intervalSecs\": 30}," +
                "\"tags\": {}," +
                "\"component\": {\"global\": {}}" +
                "}";
    }

    private static String loggingConfigJsonWithFile(String logFilePath) {
        return "{" +
                "\"logging\": {" +
                "\"level\": \"DEBUG\"," +
                "\"java_format\": \"%d [%level] %logger - %msg%n\"," +
                "\"fileLogging\": {\"enabled\": true, \"filePath\": \"" + logFilePath + "\"}," +
                "\"loggers\": {\"com.mbreissi\": \"INFO\"}," +
                "\"globalControl\": true" +
                "}," +
                "\"metricEmission\": {\"target\": \"log\"}," +
                "\"heartbeat\": {\"intervalSecs\": 30}," +
                "\"tags\": {}," +
                "\"component\": {\"global\": {}}" +
                "}";
    }

    private static ConfigManager configManagerFor(String configJson) throws IOException {
        File tempFile = File.createTempFile("global-logging-config", ".json");
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
        } catch (Exception e) {
            throw new RuntimeException("Failed to create ConfigManager: " + e.getMessage(), e);
        }
    }
}
