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
 * Unit tests for {@link LoggingConfiguration} parsing, focusing on the file-rotation
 * settings ({@code maxFileSize} / {@code backupCount}) added for parity with the
 * Python/Rust libraries.
 */
class LoggingConfigurationTest {

    private static LoggingConfiguration parse(String json) {
        return new LoggingConfiguration(JsonParser.parseString(json).getAsJsonObject());
    }

    @Test
    void rotationDefaultsWhenOmitted() {
        LoggingConfiguration cfg = parse("""
                {"fileLogging": {"enabled": true, "filePath": "/var/log/x.log"}}""");
        assertTrue(cfg.isFileLoggingEnabled());
        assertEquals("10MB", cfg.getMaxFileSize());
        assertEquals(5, cfg.getBackupCount());
    }

    @Test
    void rotationValuesParsedWhenPresent() {
        LoggingConfiguration cfg = parse("""
            {"fileLogging": {"enabled": true, "filePath": "/var/log/x.log", \
            "maxFileSize": "512KB", "backupCount": 3}}""");
        assertEquals("512KB", cfg.getMaxFileSize());
        assertEquals(3, cfg.getBackupCount());
    }

    @Test
    void rotationDefaultsWhenNoFileLoggingSection() {
        LoggingConfiguration cfg = parse("""
                {"level": "INFO"}""");
        assertFalse(cfg.isFileLoggingEnabled());
        assertEquals("10MB", cfg.getMaxFileSize());
        assertEquals(5, cfg.getBackupCount());
    }

    @Test
    void toDictIncludesRotationSettingsWhenEnabled() {
        LoggingConfiguration cfg = parse("""
            {"fileLogging": {"enabled": true, "filePath": "/var/log/x.log", \
            "maxFileSize": "1GB", "backupCount": 2}}""");
        JsonObject dict = cfg.toDict();
        assertTrue(dict.has("fileLogging"));
        JsonObject fileLogging = dict.getAsJsonObject("fileLogging");
        assertEquals("1GB", fileLogging.get("maxFileSize").getAsString());
        assertEquals(2, fileLogging.get("backupCount").getAsInt());
    }
}
