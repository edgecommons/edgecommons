/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config;

import com.breissinger.ggcommons.ParsedCommandLine;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Integration test for size-based log-file rotation. Drives the real
 * {@link ConfigManager#reconfigureLogging()} path (invoked during config load) so
 * that Log4j2 builds a {@code RollingFile} appender, then logs enough volume to
 * force several rollovers and asserts that backup files are produced and capped at
 * {@code backupCount} — the parity contract shared with the Python/Rust libraries.
 */
class FileLoggingRotationTest {

    @Test
    void rollsOverBySizeAndCapsBackups() throws IOException {
        File logDir = Files.createTempDirectory("ggcommons-rotate").toFile();
        logDir.deleteOnExit();
        String logFilePath = new File(logDir, "app.log").getAbsolutePath().replace("\\", "/");

        // Tiny maxFileSize + backupCount=2 so a modest log volume forces many
        // rollovers and exercises the backup cap.
        String configJson = "{" +
                "\"logging\": {" +
                "\"level\": \"INFO\"," +
                "\"java_format\": \"%m%n\"," +
                "\"fileLogging\": {\"enabled\": true, \"filePath\": \"" + logFilePath + "\", " +
                "\"maxFileSize\": \"1KB\", \"backupCount\": 2}" +
                "}," +
                "\"metricEmission\": {\"target\": \"log\"}," +
                "\"heartbeat\": {\"intervalSecs\": 30}," +
                "\"tags\": {}," +
                "\"component\": {\"global\": {}}" +
                "}";

        // Constructing the ConfigManager applies the config and reconfigures logging,
        // installing the RollingFile appender on the global Log4j2 context.
        createConfigManager(configJson);

        Logger logger = LogManager.getLogger(FileLoggingRotationTest.class);
        for (int i = 0; i < 2000; i++) {
            logger.info("rotation padding line number {} with some extra width to grow the file", i);
        }

        File base = new File(logDir, "app.log");
        File backup1 = new File(logDir, "app.log.1");
        File backup2 = new File(logDir, "app.log.2");
        File backup3 = new File(logDir, "app.log.3");

        assertTrue(base.exists(), "active log file must exist");
        assertTrue(backup1.exists(), "rotation must have produced at least one backup");
        assertTrue(backup2.exists(), "second backup must exist with backupCount=2");
        assertFalse(backup3.exists(), "backupCount=2 must not keep a third backup");
    }

    private static void createConfigManager(String configJson) throws IOException {
        File tempFile = File.createTempFile("rotation-config", ".json");
        tempFile.deleteOnExit();
        try (FileWriter writer = new FileWriter(tempFile)) {
            writer.write(configJson);
            writer.flush();
        }
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", tempFile.getAbsolutePath()};
        cmdLine.thingName = "test-thing";
        try {
            ConfigManagerFactory.create("com.test.TestComponent", cmdLine);
        } catch (Exception e) {
            throw new RuntimeException("Failed to create ConfigManager: " + e.getMessage(), e);
        }
    }
}
