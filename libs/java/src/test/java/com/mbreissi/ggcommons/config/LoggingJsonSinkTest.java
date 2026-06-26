/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config;

import com.mbreissi.ggcommons.ParsedCommandLine;
import com.mbreissi.ggcommons.platform.Platform;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.core.Appender;
import org.apache.logging.log4j.core.Layout;
import org.apache.logging.log4j.core.LogEvent;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.Configurator;
import org.apache.logging.log4j.core.impl.Log4jLogEvent;
import org.apache.logging.log4j.core.layout.PatternLayout;
import org.apache.logging.log4j.message.SimpleMessage;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Path;
import java.util.LinkedHashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Phase 1c logging slice (FR-LOG-1..4): the structured stdout-JSON logging sink, its selection via
 * the {@code logging.java_format} {@code json} token, the KUBERNETES-platform default + precedence,
 * the no-in-process-rotation guarantee under the JSON sink, and the best-effort correlation fields.
 *
 * <p>The pure tests exercise {@link ConfigManager#buildJsonPattern}/{@link ConfigManager#jsonField}
 * through a real Log4j2 {@link PatternLayout} + synthetic {@link LogEvent} (no global state). The
 * precedence / no-rotation tests drive the real {@link ConfigManager#reconfigureLogging()} via the
 * factory and inspect the live Log4j2 configuration; an {@link AfterEach} restores default logging so
 * the JSON sink never leaks into unrelated tests.
 */
class LoggingJsonSinkTest {

    @AfterEach
    void restoreDefaultLogging() {
        // reconfigureLogging() mutates the process-global Log4j2 context; reset it so a JSON-mode
        // configuration left behind by a test does not affect other suites.
        Configurator.reconfigure();
    }

    // ---------- FR-LOG-1: one valid JSON object per line ----------

    /**
     * Serialize an event through the JSON pattern and return the raw (line-terminated) output. Mirrors
     * the production layout config in {@link ConfigManager#reconfigureLogging()}:
     * {@code alwaysWriteExceptions=false} so PatternLayout does not append a raw stack trace after the
     * JSON object (the pattern renders the exception itself).
     */
    private static String renderJson(Map<String, String> correlation, LogEvent event) {
        PatternLayout layout = PatternLayout.newBuilder()
                .withPattern(ConfigManager.buildJsonPattern(correlation))
                .withAlwaysWriteExceptions(false)
                .build();
        return layout.toSerializable(event);
    }

    private static LogEvent event(Level level, String logger, String message, Throwable thrown) {
        return Log4jLogEvent.newBuilder()
                .setLoggerName(logger)
                .setLevel(level)
                .setMessage(new SimpleMessage(message))
                .setThrown(thrown)
                .build();
    }

    @Test
    void jsonSinkEmitsValidSingleObjectWithRequiredFields() {
        String out = renderJson(Map.of(), event(Level.INFO, "com.example.Foo", "hello world", null));

        // Exactly one physical line (one JSON object per line; the trailing newline is the only one).
        assertTrue(out.endsWith(System.lineSeparator()) || out.endsWith("\n"), "line must be terminated");
        String line = out.strip();
        assertEquals(-1, line.indexOf('\n'), "must be a single physical line");

        JsonObject obj = JsonParser.parseString(line).getAsJsonObject(); // throws if not valid JSON
        assertTrue(obj.has("timestamp"), "timestamp field required");
        assertEquals("INFO", obj.get("level").getAsString());
        assertEquals("com.example.Foo", obj.get("logger").getAsString());
        assertEquals("hello world", obj.get("message").getAsString());
        assertFalse(obj.has("thrown"), "no exception -> no thrown field");
    }

    @Test
    void jsonSinkEscapesMessageWithQuotesNewlinesAndStaysOneLine() {
        // A message with quotes, a newline and a tab must be JSON-escaped onto a single line.
        String out = renderJson(Map.of(), event(Level.WARN, "L", "he said \"hi\"\nline2\tend", null));
        String line = out.strip();
        assertEquals(-1, line.indexOf('\n'), "embedded newline must be escaped, not split the line");

        JsonObject obj = JsonParser.parseString(line).getAsJsonObject();
        assertEquals("he said \"hi\"\nline2\tend", obj.get("message").getAsString(),
                "round-trip through JSON must preserve the original message exactly");
    }

    @Test
    void jsonSinkIncludesThrownOnlyWhenExceptionPresent() {
        // No exception -> field absent.
        JsonObject noEx = JsonParser.parseString(
                renderJson(Map.of(), event(Level.ERROR, "L", "ok", null)).strip()).getAsJsonObject();
        assertFalse(noEx.has("thrown"));

        // Exception present -> a single-line, JSON-escaped stack trace under "thrown".
        String out = renderJson(Map.of(),
                event(Level.ERROR, "L", "boom", new RuntimeException("kaboom \"q\"")));
        String line = out.strip();
        assertEquals(-1, line.indexOf('\n'), "stack trace must be escaped onto one line");
        JsonObject obj = JsonParser.parseString(line).getAsJsonObject();
        assertTrue(obj.has("thrown"), "exception present -> thrown field");
        String thrown = obj.get("thrown").getAsString();
        assertTrue(thrown.contains("RuntimeException"), "thrown carries the throwable class");
        assertTrue(thrown.contains("kaboom \"q\""), "thrown carries the (escaped) message");
    }

    // ---------- FR-LOG-3: correlation fields present / absent ----------

    @Test
    void jsonSinkIncludesCorrelationWhenPresentAndOmitsWhenAbsent() {
        Map<String, String> corr = new LinkedHashMap<>();
        corr.put("thing", "thing-1");
        corr.put("pod", "pod-7");
        corr.put("namespace", "team-a");
        corr.put("node", "node-1");
        JsonObject withCorr = JsonParser.parseString(
                renderJson(corr, event(Level.INFO, "L", "m", null)).strip()).getAsJsonObject();
        assertEquals("thing-1", withCorr.get("thing").getAsString());
        assertEquals("pod-7", withCorr.get("pod").getAsString());
        assertEquals("team-a", withCorr.get("namespace").getAsString());
        assertEquals("node-1", withCorr.get("node").getAsString());

        // No correlation supplied -> none of the fields appear (no empty/null noise).
        JsonObject none = JsonParser.parseString(
                renderJson(Map.of(), event(Level.INFO, "L", "m", null)).strip()).getAsJsonObject();
        assertFalse(none.has("thing"));
        assertFalse(none.has("pod"));
        assertFalse(none.has("namespace"));
        assertFalse(none.has("node"));
    }

    @Test
    void jsonSinkStaysValidWithHostileCorrelationValue() {
        // A correlation value carrying JSON-special chars (quote, backslash, newline, %) must still
        // yield valid JSON; the unsafe characters are neutralized to '_' (best-effort identifiers).
        Map<String, String> corr = new LinkedHashMap<>();
        corr.put("thing", "a\"b\\c\nd%e");
        JsonObject obj = JsonParser.parseString(
                renderJson(corr, event(Level.INFO, "L", "m", null)).strip()).getAsJsonObject();
        assertEquals("a_b_c_d_e", obj.get("thing").getAsString());
    }

    // ---------- jsonField / sanitizeForJsonLiteral units ----------

    @Test
    void jsonFieldEmptyForNullOrEmptyValue() {
        assertEquals("", ConfigManager.jsonField("pod", null));
        assertEquals("", ConfigManager.jsonField("pod", ""));
    }

    @Test
    void jsonFieldEmitsCleanLiteralForSafeValue() {
        // The common case: a k8s-style identifier passes through unchanged.
        assertEquals(",\"pod\":\"my-pod-123.ns\"", ConfigManager.jsonField("pod", "my-pod-123.ns"));
    }

    @Test
    void sanitizeForJsonLiteralNeutralizesUnsafeChars() {
        // %, ", \, and control chars (which break JSON or are PatternLayout-special) become '_'.
        assertEquals("a_b", ConfigManager.sanitizeForJsonLiteral("a%b"));
        assertEquals("x_y_z", ConfigManager.sanitizeForJsonLiteral("x\"y\\z"));
        assertEquals("a_b_c", ConfigManager.sanitizeForJsonLiteral("a\nb\tc"));
        assertEquals("safe.id-1", ConfigManager.sanitizeForJsonLiteral("safe.id-1"));
    }

    // ---------- FR-LOG-4 / FR-RT-3: selection + precedence (effective format) ----------

    @Test
    void kubernetesDefaultsToJsonSink(@TempDir Path dir) throws IOException {
        // FR-LOG-1: a KUBERNETES pod with no logging.java_format defaults to the json sink.
        ConfigManager cm = createConfigManager(dir, Platform.KUBERNETES, configWithLogging("\"level\":\"INFO\""), "k8s-thing");
        assertEquals("json", cm.resolveEffectiveLogFormat());
        assertTrue(consoleEmitsJson(), "the live Console appender must emit JSON on KUBERNETES");
    }

    @Test
    void explicitConfigFormatOverridesKubernetesJsonDefault(@TempDir Path dir) throws IOException {
        // FR-RT-3: an explicit (non-json) java_format wins over the platform-profile json default.
        ConfigManager cm = createConfigManager(dir, Platform.KUBERNETES,
                configWithLogging("\"java_format\":\"%m%n\""), "k8s-thing");
        assertEquals("%m%n", cm.resolveEffectiveLogFormat());
        assertFalse(consoleEmitsJson(), "an explicit text format must not produce the JSON sink");
    }

    @Test
    void explicitJsonTokenSelectsSinkOffKubernetes(@TempDir Path dir) throws IOException {
        // FR-LOG-4: the json token (case-insensitive) selects the sink on any platform.
        ConfigManager cm = createConfigManager(dir, Platform.HOST,
                configWithLogging("\"java_format\":\"JSON\""), "host-thing");
        assertEquals("JSON", cm.resolveEffectiveLogFormat());
        assertTrue(consoleEmitsJson(), "explicit json token selects the JSON sink even on HOST");
    }

    @Test
    void hostDefaultIsUnchangedTextSink(@TempDir Path dir) throws IOException {
        ConfigManager cm = createConfigManager(dir, Platform.HOST, configWithLogging("\"level\":\"INFO\""), "host-thing");
        assertEquals(LoggingConfiguration.DEFAULT_FORMAT, cm.resolveEffectiveLogFormat());
        assertFalse(consoleEmitsJson(), "HOST default must remain the console/text sink");
    }

    @Test
    void greengrassDefaultIsUnchangedTextSink(@TempDir Path dir) throws IOException {
        ConfigManager cm = createConfigManager(dir, Platform.GREENGRASS, configWithLogging("\"level\":\"INFO\""), "gg-thing");
        assertEquals(LoggingConfiguration.DEFAULT_FORMAT, cm.resolveEffectiveLogFormat());
        assertFalse(consoleEmitsJson());
    }

    @Test
    void nullPlatformDefaultIsUnchangedTextSink(@TempDir Path dir) throws IOException {
        // No resolved platform (test/subclass bring-up) -> the library default, never json.
        ConfigManager cm = createConfigManager(dir, null, configWithLogging("\"level\":\"INFO\""), "thing");
        assertEquals(LoggingConfiguration.DEFAULT_FORMAT, cm.resolveEffectiveLogFormat());
    }

    @Test
    void kubernetesThingCorrelationAppearsInEmittedJson(@TempDir Path dir) throws IOException {
        // End-to-end: the resolved identity is emitted as the `thing` correlation field on the JSON sink.
        createConfigManager(dir, Platform.KUBERNETES, configWithLogging("\"level\":\"INFO\""), "pod-xyz");
        JsonObject obj = JsonParser.parseString(consoleLayoutOutput().strip()).getAsJsonObject();
        assertEquals("pod-xyz", obj.get("thing").getAsString());
    }

    // ---------- FR-LOG-2: no in-process rotation under the JSON sink ----------

    @Test
    void kubernetesJsonSinkInstallsNoRollingFileEvenWithFileLoggingEnabled(@TempDir Path dir) throws IOException {
        String logFile = new File(dir.toFile(), "app.log").getAbsolutePath().replace("\\", "/");
        String logging = "\"level\":\"INFO\",\"fileLogging\":{\"enabled\":true,\"filePath\":\"" + logFile + "\"}";
        createConfigManager(dir, Platform.KUBERNETES, configWithLogging(logging), "k8s-thing");

        Configuration cfg = ((LoggerContext) LogManager.getContext(false)).getConfiguration();
        assertTrue(cfg.getAppenders().containsKey("Console"), "Console (stdout) appender must exist");
        assertFalse(cfg.getAppenders().containsKey("File"),
                "FR-LOG-2: no RollingFile appender under the k8s JSON default, even with fileLogging enabled");
    }

    @Test
    void hostKeepsRollingFileWhenFileLoggingEnabled(@TempDir Path dir) throws IOException {
        // Off the JSON sink, fileLogging still installs the RollingFile appender (behavior unchanged).
        String logFile = new File(dir.toFile(), "app.log").getAbsolutePath().replace("\\", "/");
        String logging = "\"level\":\"INFO\",\"fileLogging\":{\"enabled\":true,\"filePath\":\"" + logFile + "\"}";
        createConfigManager(dir, Platform.HOST, configWithLogging(logging), "host-thing");

        Configuration cfg = ((LoggerContext) LogManager.getContext(false)).getConfiguration();
        assertTrue(cfg.getAppenders().containsKey("File"), "HOST file logging keeps the RollingFile appender");
    }

    // ---------- globalControl=true path must honor the JSON sink identically ----------

    @Test
    void globalControlKubernetesDefaultsToJsonSinkWithNoFileAppender(@TempDir Path dir) throws IOException {
        // Regression: reconfigureLogging() short-circuits to GlobalLoggingManager when
        // logging.globalControl=true. That path must ALSO default to the JSON sink on KUBERNETES and
        // install NO RollingFile appender (FR-LOG-1/2), matching the non-global path — not bypass it.
        String logFile = new File(dir.toFile(), "app.log").getAbsolutePath().replace("\\", "/");
        String logging = "\"level\":\"INFO\",\"globalControl\":true,"
                + "\"fileLogging\":{\"enabled\":true,\"filePath\":\"" + logFile + "\"}";
        createConfigManager(dir, Platform.KUBERNETES, configWithLogging(logging), "k8s-thing");

        assertTrue(consoleEmitsJson(), "globalControl=true on KUBERNETES must still emit the JSON sink");
        Configuration cfg = ((LoggerContext) LogManager.getContext(false)).getConfiguration();
        assertFalse(cfg.getAppenders().containsKey("File"),
                "FR-LOG-2: the globalControl JSON sink installs no RollingFile appender");
    }

    @Test
    void globalControlExplicitJsonEmitsValidJsonNotLiteralPattern(@TempDir Path dir) throws IOException {
        // FR-LOG-4 on the globalControl path: java_format="json" must select the JSON sink, not be fed
        // to Log4j2 as the literal PatternLayout pattern "json" (which would print the word "json").
        createConfigManager(dir, Platform.HOST,
                configWithLogging("\"java_format\":\"json\",\"globalControl\":true"), "host-thing");
        assertTrue(consoleEmitsJson(), "globalControl + java_format=json must emit valid JSON");
    }

    @Test
    void globalControlHostDefaultStaysTextSink(@TempDir Path dir) throws IOException {
        // No regression off-k8s: globalControl=true on HOST with no java_format keeps the text sink.
        createConfigManager(dir, Platform.HOST,
                configWithLogging("\"level\":\"INFO\",\"globalControl\":true"), "host-thing");
        assertFalse(consoleEmitsJson(), "globalControl on HOST keeps the console/text sink by default");
    }

    // ---------- helpers ----------

    /** Whether the live Console appender's layout serializes a sample event to a JSON object. */
    private static boolean consoleEmitsJson() {
        String out = consoleLayoutOutput();
        if (out == null) {
            return false;
        }
        try {
            return JsonParser.parseString(out.strip()).isJsonObject();
        } catch (RuntimeException e) {
            return false;
        }
    }

    /** Serialize a sample INFO event through the live Console appender's layout. */
    private static String consoleLayoutOutput() {
        Configuration cfg = ((LoggerContext) LogManager.getContext(false)).getConfiguration();
        Appender console = cfg.getAppenders().get("Console");
        if (console == null) {
            return null;
        }
        Layout<?> layout = console.getLayout();
        Object serialized = layout.toSerializable(event(Level.INFO, "com.test.Sample", "sample", null));
        return serialized == null ? null : serialized.toString();
    }

    private static String configWithLogging(String loggingBody) {
        return "{"
                + "\"logging\":{" + loggingBody + "},"
                + "\"metricEmission\":{\"target\":\"log\"},"
                + "\"heartbeat\":{\"intervalSecs\":30},"
                + "\"tags\":{},"
                + "\"component\":{\"global\":{}}"
                + "}";
    }

    private static ConfigManager createConfigManager(Path dir, Platform platform, String configJson, String thing)
            throws IOException {
        File cfgFile = File.createTempFile("logging-json", ".json", dir.toFile());
        try (FileWriter w = new FileWriter(cfgFile)) {
            w.write(configJson);
            w.flush();
        }
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", cfgFile.getAbsolutePath()};
        cmdLine.platform = platform;
        cmdLine.thingName = thing;
        try {
            return ConfigManagerFactory.create("com.test.TestComponent", cmdLine);
        } catch (Exception e) {
            throw new RuntimeException("Failed to create ConfigManager: " + e.getMessage(), e);
        }
    }
}
