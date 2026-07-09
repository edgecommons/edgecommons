/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

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

    @Test
    void publishDefaultsMatchLogBusSpec() {
        LoggingConfiguration cfg = parse("{}");
        LoggingConfiguration.LogPublishConfiguration publish = cfg.getPublishConfig();
        assertFalse(publish.isEnabled());
        assertEquals(LoggingConfiguration.LogPublishConfiguration.Destination.LOCAL,
                publish.getDestination());
        assertEquals("INFO", publish.getMinLevel());
        assertTrue(publish.isCaptureNative());
        assertFalse(publish.isCaptureConsole());
        assertEquals(8192, publish.getMaxRecordBytes());
        assertEquals(1000, publish.getQueueMaxRecords());
        assertEquals(LoggingConfiguration.LogPublishConfiguration.QueueOnFull.DROP_OLDEST,
                publish.getQueueOnFull());
        assertTrue(publish.isRedactionEnabled());
        assertEquals("***", publish.getRedactionReplacement());
        assertTrue(publish.getRedactionExtraPatterns().isEmpty());
    }

    @Test
    void publishConfigParsesPopulatedSection() {
        LoggingConfiguration cfg = parse("""
            {"publish":{"enabled":true,"destination":"northbound","minLevel":"DEBUG",\
            "captureNative":false,"captureConsole":true,"maxRecordBytes":4096,\
            "queue":{"maxRecords":25,"onFull":"dropOldest"},\
            "redaction":{"enabled":false,"replacement":"[x]","extraPatterns":["abc"]}}}""");
        LoggingConfiguration.LogPublishConfiguration publish = cfg.getPublishConfig();
        assertTrue(publish.isEnabled());
        assertEquals(LoggingConfiguration.LogPublishConfiguration.Destination.NORTHBOUND,
                publish.getDestination());
        assertEquals("DEBUG", publish.getMinLevel());
        assertFalse(publish.isCaptureNative());
        assertTrue(publish.isCaptureConsole());
        assertEquals(4096, publish.getMaxRecordBytes());
        assertEquals(25, publish.getQueueMaxRecords());
        assertFalse(publish.isRedactionEnabled());
        assertEquals("[x]", publish.getRedactionReplacement());
        assertEquals(java.util.List.of("abc"), publish.getRedactionExtraPatterns());
    }

    @Test
    void publishConfigRejectsInvalidEnums() {
        assertThrows(IllegalArgumentException.class,
                () -> parse("{\"publish\":{\"destination\":\"remote\"}}"));
        assertThrows(IllegalArgumentException.class,
                () -> parse("{\"publish\":{\"minLevel\":\"NOTICE\"}}"));
        assertThrows(IllegalArgumentException.class,
                () -> parse("{\"publish\":{\"queue\":{\"onFull\":\"block\"}}}"));
    }
}
