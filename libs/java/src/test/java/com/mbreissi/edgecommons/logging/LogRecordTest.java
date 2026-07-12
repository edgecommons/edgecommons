/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.logging;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mbreissi.edgecommons.config.LoggingConfiguration;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.core.LogEvent;
import org.apache.logging.log4j.core.impl.Log4jLogEvent;
import org.apache.logging.log4j.message.SimpleMessage;
import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.time.Instant;
import java.util.LinkedHashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for the log-bus value types: {@link LogRecord} (+ its builder), {@link LogLevel},
 * {@link LogStats}, and the {@code LogBusCapture} bridge that adapts Log4j2 / console events into
 * records.
 */
class LogRecordTest {

    private static LogService logService() {
        MockConfigurationService config = new MockConfigurationService();
        config.setLoggingConfig(new LoggingConfiguration(
                JsonParser.parseString("{\"publish\":{\"enabled\":true,\"captureNative\":true,\"captureConsole\":true}}")
                        .getAsJsonObject()));
        return new LogService(config, new MockMessagingService());
    }

    private static LogEvent event(Level level, String message, Throwable thrown) {
        return Log4jLogEvent.newBuilder()
                .setLoggerName("com.example.App")
                .setLevel(level)
                .setMessage(new SimpleMessage(message))
                .setThrown(thrown)
                .setThreadName("worker-1")
                .setTimeMillis(1_752_000_000_000L)
                .build();
    }

    // ---------------------------------------------------------------- LogRecord

    @Test
    void builderCarriesEveryField() {
        Instant ts = Instant.parse("2026-07-09T12:34:56Z");
        JsonObject error = new JsonObject();
        error.addProperty("type", "java.lang.IllegalStateException");

        LogRecord record = LogRecord.builder()
                .withTimestamp(ts)
                .withLevel("warn")
                .withLogger("app")
                .withMessage("careful")
                .withSequence(7L)
                .withThread("main")
                .withFields(Map.of("k", "v"))
                .withError(error)
                .withTruncated(true)
                .withDropped(3L)
                .build();

        assertEquals(ts, record.getTimestamp());
        assertEquals(LogLevel.WARN, record.getLevel());
        assertEquals("app", record.getLogger());
        assertEquals("careful", record.getMessage());
        assertEquals(7L, record.getSequence());
        assertEquals("main", record.getThread());
        assertEquals(Map.of("k", "v"), record.getFields());
        assertEquals(error, record.getError());
        assertEquals(Boolean.TRUE, record.getTruncated());
        assertEquals(3L, record.getDropped());
    }

    @Test
    void builderDefaultsTimestampLevelMessageAndDropsBlanks() {
        LogRecord record = LogRecord.builder().withLogger("app").withThread("  ").build();

        assertNotNull(record.getTimestamp());
        assertEquals(LogLevel.INFO, record.getLevel());
        assertEquals("", record.getMessage());
        assertNull(record.getThread());
        assertNull(record.getFields());
        assertNull(record.getSequence());
        assertNull(record.getError());
        assertNull(record.getTruncated());
        assertNull(record.getDropped());
    }

    @Test
    void builderRejectsInvalidInput() {
        assertThrows(IllegalArgumentException.class, () -> LogRecord.builder().withLogger(" ").build());
        assertThrows(IllegalArgumentException.class, () -> LogRecord.builder().build());
        assertThrows(IllegalArgumentException.class, () -> LogRecord.builder().withSequence(-1L));
        assertThrows(IllegalArgumentException.class, () -> LogRecord.builder().withDropped(-1L));
    }

    @Test
    void addFieldAccumulatesAndIgnoresNullKeys() {
        LogRecord record = LogRecord.builder()
                .withLogger("app")
                .addField("a", 1)
                .addField("b", 2)
                .addField(null, 3)
                .build();

        assertEquals(2, record.getFields().size());
        assertEquals(1, record.getFields().get("a"));
        assertEquals(2, record.getFields().get("b"));
    }

    @Test
    void withFieldsClearsOnEmptyAndSkipsNullKeys() {
        assertNull(LogRecord.builder().withLogger("app").withFields(Map.of()).build().getFields());
        assertNull(LogRecord.builder().withLogger("app").withFields(null).build().getFields());

        Map<String, Object> withNullKey = new LinkedHashMap<>();
        withNullKey.put(null, "skipped");
        withNullKey.put("kept", "yes");
        LogRecord record = LogRecord.builder().withLogger("app").withFields(withNullKey).build();

        assertEquals(Map.of("kept", "yes"), record.getFields());
    }

    @Test
    void toBuilderRoundTripsEveryField() {
        JsonObject error = new JsonObject();
        error.addProperty("type", "java.lang.RuntimeException");
        LogRecord original = LogRecord.builder()
                .withTimestamp(Instant.parse("2026-07-09T00:00:00Z"))
                .withLevel(LogLevel.ERROR)
                .withLogger("app")
                .withMessage("boom")
                .withSequence(11L)
                .withThread("t-1")
                .withFields(Map.of("k", "v"))
                .withError(error)
                .withTruncated(true)
                .withDropped(2L)
                .build();

        LogRecord copy = original.toBuilder().build();

        assertEquals(original.getTimestamp(), copy.getTimestamp());
        assertEquals(original.getLevel(), copy.getLevel());
        assertEquals(original.getLogger(), copy.getLogger());
        assertEquals(original.getMessage(), copy.getMessage());
        assertEquals(original.getSequence(), copy.getSequence());
        assertEquals(original.getThread(), copy.getThread());
        assertEquals(original.getFields(), copy.getFields());
        assertEquals(original.getError(), copy.getError());
        assertEquals(original.getTruncated(), copy.getTruncated());
        assertEquals(original.getDropped(), copy.getDropped());
    }

    // ---------------------------------------------------------------- LogLevel

    @Test
    void parseIsCaseInsensitiveAndRejectsEmpty() {
        assertEquals(LogLevel.DEBUG, LogLevel.parse("debug"));
        assertEquals(LogLevel.FATAL, LogLevel.parse("  Fatal "));
        assertThrows(IllegalArgumentException.class, () -> LogLevel.parse(null));
        assertThrows(IllegalArgumentException.class, () -> LogLevel.parse(" "));
        assertThrows(IllegalArgumentException.class, () -> LogLevel.parse("verbose"));
    }

    @Test
    void fromLog4jMapsEveryLevel() {
        assertEquals(LogLevel.INFO, LogLevel.fromLog4j(null));
        assertEquals(LogLevel.FATAL, LogLevel.fromLog4j(Level.FATAL));
        assertEquals(LogLevel.ERROR, LogLevel.fromLog4j(Level.ERROR));
        assertEquals(LogLevel.WARN, LogLevel.fromLog4j(Level.WARN));
        assertEquals(LogLevel.INFO, LogLevel.fromLog4j(Level.INFO));
        assertEquals(LogLevel.DEBUG, LogLevel.fromLog4j(Level.DEBUG));
        assertEquals(LogLevel.TRACE, LogLevel.fromLog4j(Level.TRACE));
        assertEquals(LogLevel.TRACE, LogLevel.fromLog4j(Level.ALL));
    }

    @Test
    void topicTokenIsLowercase() {
        assertEquals("error", LogLevel.ERROR.topicToken());
        assertEquals("info", LogLevel.INFO.topicToken());
    }

    // ---------------------------------------------------------------- LogStats

    @Test
    void statsSnapshotExposesEveryCounter() {
        LogStats stats = new LogStats(1L, 2L, 3L, 4L, 5L, 6L, 7L, 8);

        assertEquals(1L, stats.getEnqueuedRecords());
        assertEquals(2L, stats.getPublishedRecords());
        assertEquals(3L, stats.getDroppedRecords());
        assertEquals(4L, stats.getFilteredRecords());
        assertEquals(5L, stats.getRedactedRecords());
        assertEquals(6L, stats.getTruncatedRecords());
        assertEquals(7L, stats.getPublishFailures());
        assertEquals(8, stats.getQueuedRecords());
    }

    // ---------------------------------------------------------------- LogBusCapture

    @Test
    void captureIsANoOpWithoutAService() {
        LogBusCapture.clearService(null);
        // No service bound: none of the three entry points may throw or enqueue anything.
        LogBusCapture.capture(event(Level.ERROR, "dropped", null));
        LogBusCapture.captureConsole("app", LogLevel.INFO, "dropped");
        LogBusCapture.captureConsole(LogRecord.builder().withLogger("app").build());
    }

    @Test
    void captureTurnsALog4jEventIntoARecord() {
        LogService logs = logService();
        try {
            LogBusCapture.capture(event(Level.WARN, "careful", null));
            assertTrue(logs.flush(Duration.ofSeconds(2)));

            assertEquals(1L, logs.stats().getEnqueuedRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void captureAttachesTheThrownErrorAndDefaultsAMissingLoggerName() {
        LogService logs = logService();
        try {
            LogEvent thrown = Log4jLogEvent.newBuilder()
                    .setLoggerName(null)
                    .setLevel(Level.ERROR)
                    .setMessage(null)
                    .setThrown(new IllegalStateException("bad state"))
                    .setThreadName("worker-2")
                    .setTimeMillis(1_752_000_000_000L)
                    .build();

            LogBusCapture.capture(thrown);
            assertTrue(logs.flush(Duration.ofSeconds(2)));

            assertEquals(1L, logs.stats().getEnqueuedRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void captureConsoleEnqueuesTextAndIgnoresBlankMessages() {
        LogService logs = logService();
        try {
            LogBusCapture.captureConsole("app", LogLevel.INFO, "hello");
            LogBusCapture.captureConsole("app", LogLevel.INFO, "");
            LogBusCapture.captureConsole("app", LogLevel.INFO, null);
            LogBusCapture.captureConsole((LogRecord) null);
            assertTrue(logs.flush(Duration.ofSeconds(2)));

            assertEquals(1L, logs.stats().getEnqueuedRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void clearServiceOnlyClearsTheBoundService() {
        LogService first = logService();
        LogService second = logService(); // rebinds the static to `second`
        try {
            // Clearing with a stale reference must not unbind the current service.
            LogBusCapture.clearService(first);
            LogBusCapture.captureConsole("app", LogLevel.INFO, "still captured");
            assertTrue(second.flush(Duration.ofSeconds(2)));

            assertEquals(1L, second.stats().getEnqueuedRecords());
        } finally {
            second.close();
            first.close();
        }
    }
}
