package com.mbreissi.edgecommons.logging;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mbreissi.edgecommons.ParsedCommandLine;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigManagerFactory;
import com.mbreissi.edgecommons.config.LoggingConfiguration;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.platform.Platform;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.config.Configurator;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.*;

class LogServiceTest {
    private static final String LOG_TOPIC_INFO = "ecv1/test-thing/TestComponent/log/info";
    private static final org.apache.logging.log4j.Logger PRECREATED_APP_LOGGER =
            LogManager.getLogger("com.mbreissi.edgecommons.opcua.opc.OpcUaConnection");

    private static LoggingConfiguration logging(String publishJson) {
        return new LoggingConfiguration(JsonParser.parseString("{\"publish\":" + publishJson + "}")
                .getAsJsonObject());
    }

    private static MockConfigurationService config(String publishJson) {
        MockConfigurationService config = new MockConfigurationService();
        config.setLoggingConfig(logging(publishJson));
        return config;
    }

    /**
     * The body of the first published record whose {@code logger} equals {@code loggerName}.
     *
     * <p>The native {@link LogBusAppender} captures the whole shared log4j LoggerContext, so an
     * unrelated logger active in the surefire JVM (for example a config {@code FileWatcher} thread
     * left running by another test class) can publish a record ahead of the one under test.
     * Selecting by logger keeps these native-capture assertions independent of cross-test emission
     * order instead of assuming the record under test is published first.
     */
    private static JsonObject publishedBodyForLogger(MockMessagingService messaging, String loggerName) {
        return messaging.getPublishedMessages().stream()
                .map(published -> published.message.toDict().getAsJsonObject("body"))
                .filter(body -> body.has("logger") && loggerName.equals(body.get("logger").getAsString()))
                .findFirst()
                .orElseThrow(() -> new AssertionError(
                        "no published log record from logger '" + loggerName + "'; published loggers="
                                + messaging.getPublishedMessages().stream()
                                        .map(published -> published.message.toDict()
                                                .getAsJsonObject("body").get("logger"))
                                        .toList()));
    }

    @Test
    void explicitPublishUsesReservedLogTopicAndEnvelopeShape() {
        MockConfigurationService config = config("{\"enabled\":true}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            Instant ts = Instant.parse("2026-07-09T12:34:56Z");
            logs.publish(LogRecord.builder()
                    .withTimestamp(ts)
                    .withLevel(LogLevel.INFO)
                    .withLogger("app")
                    .withMessage("started")
                    .withThread("main")
                    .withFields(Map.of("unit", "pump-1"))
                    .build());

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
            assertEquals(1, published.size());
            assertEquals(LOG_TOPIC_INFO, published.get(0).topic);
            assertTrue(published.get(0).reserved);
            Message message = published.get(0).message;
            assertEquals("log", message.getHeader().getName());
            assertEquals("1.0", message.getHeader().getVersion());
            assertEquals(ts.toString(), message.getHeader().getTimestamp());

            JsonObject body = message.toDict().getAsJsonObject("body");
            assertEquals("edgecommons.log.v1", body.get("schema").getAsString());
            assertEquals(ts.toString(), body.get("timestamp").getAsString());
            assertEquals("INFO", body.get("level").getAsString());
            assertEquals("app", body.get("logger").getAsString());
            assertEquals("started", body.get("message").getAsString());
            assertEquals(1L, body.get("sequence").getAsLong());
            assertEquals("main", body.get("thread").getAsString());
            assertEquals("pump-1", body.getAsJsonObject("fields").get("unit").getAsString());
        } finally {
            logs.close();
        }
    }

    @Test
    void disabledPublishFiltersExplicitRecords() {
        MockConfigurationService config = config("{}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            logs.publish(LogRecord.builder()
                    .withLogger("app")
                    .withMessage("not published")
                    .build());
            assertTrue(logs.flush(Duration.ofMillis(100)));
            assertTrue(messaging.getPublishedMessages().isEmpty());
            assertEquals(1L, logs.stats().getFilteredRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void northboundDestinationPublishesThroughReservedNorthboundPath() {
        MockConfigurationService config = config("{\"enabled\":true,\"destination\":\"northbound\"}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            logs.publish(LogRecord.builder().withLevel("WARN").withLogger("app").withMessage("warn").build());
            assertTrue(logs.flush(Duration.ofSeconds(2)));
            MockMessagingService.PublishedMessage published = messaging.getPublishedMessages().get(0);
            assertEquals("ecv1/test-thing/TestComponent/log/warn", published.topic);
            assertTrue(published.reserved);
            assertNotNull(published.qos);
        } finally {
            logs.close();
        }
    }

    @Test
    void disconnectedTransportDropsWithoutCallingReservedPublisher() {
        MockConfigurationService config = config("{\"enabled\":true}");
        MockMessagingService messaging = new MockMessagingService();
        messaging.setConnected(false);
        LogService logs = new LogService(config, messaging);
        try {
            logs.publish(LogRecord.builder().withLevel("ERROR").withLogger("app").withMessage("offline").build());
            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertTrue(messaging.getPublishedMessages().isEmpty());
            assertEquals(1L, logs.stats().getPublishFailures());
        } finally {
            logs.close();
        }
    }

    @Test
    void redactionAppliesExtraPatternsAndUpdatesStats() {
        MockConfigurationService config = config("""
                {"enabled":true,"redaction":{"extraPatterns":["secret-[0-9]+"]}}""");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            logs.publish(LogRecord.builder()
                    .withLogger("app")
                    .withMessage("credential secret-1234")
                    .build());
            assertTrue(logs.flush(Duration.ofSeconds(2)));
            JsonObject body = messaging.getPublishedMessages().get(0).message.toDict().getAsJsonObject("body");
            assertEquals("credential ***", body.get("message").getAsString());
            assertEquals(1L, logs.stats().getRedactedRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void oversizeRecordIsTruncatedAndCounted() {
        MockConfigurationService config = config("{\"enabled\":true,\"maxRecordBytes\":220}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            logs.publish(LogRecord.builder()
                    .withLogger("app")
                    .withMessage("x".repeat(1000))
                    .build());
            assertTrue(logs.flush(Duration.ofSeconds(2)));
            JsonObject body = messaging.getPublishedMessages().get(0).message.toDict().getAsJsonObject("body");
            assertTrue(body.get("truncated").getAsBoolean());
            assertTrue(body.get("message").getAsString().length() < 1000);
            assertEquals(1L, logs.stats().getTruncatedRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void oversizeStructuredErrorIsTruncatedAndCounted() {
        MockConfigurationService config = config("{\"enabled\":true,\"maxRecordBytes\":500}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            JsonObject error = new JsonObject();
            error.addProperty("type", RuntimeException.class.getName());
            error.addProperty("message", "m".repeat(400));
            error.addProperty("stack", "s".repeat(4_000));

            logs.publish(LogRecord.builder()
                    .withLogger("app")
                    .withMessage("failed")
                    .withError(error)
                    .build());

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            JsonObject body = messaging.getPublishedMessages().get(0).message.toDict().getAsJsonObject("body");
            assertTrue(body.get("truncated").getAsBoolean());
            assertTrue(body.toString().getBytes(StandardCharsets.UTF_8).length <= 500);
            assertEquals(1L, logs.stats().getTruncatedRecords());
        } finally {
            logs.close();
        }
    }

    @Test
    void fullQueueDropsOldestWithoutBlocking() throws Exception {
        MockConfigurationService config = config("""
                {"enabled":true,"queue":{"maxRecords":2,"onFull":"dropOldest"}}""");
        BlockingMessaging messaging = new BlockingMessaging();
        LogService logs = new LogService(config, messaging);
        try {
            logs.publish(LogRecord.builder().withLogger("app").withMessage("first").build());
            assertTrue(messaging.started.await(2, TimeUnit.SECONDS));

            logs.publish(LogRecord.builder().withLogger("app").withMessage("second").build());
            logs.publish(LogRecord.builder().withLogger("app").withMessage("third").build());
            logs.publish(LogRecord.builder().withLogger("app").withMessage("fourth").build());

            messaging.release.countDown();
            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertEquals(1L, logs.stats().getDroppedRecords());
            List<String> messages = messaging.getPublishedMessages().stream()
                    .map(p -> p.message.toDict().getAsJsonObject("body").get("message").getAsString())
                    .toList();
            assertEquals(List.of("first", "third", "fourth"), messages);
            assertTrue(messaging.getPublishedMessages().stream()
                            .map(p -> p.message.toDict().getAsJsonObject("body"))
                            .anyMatch(body -> body.has("dropped")
                                    && body.get("dropped").getAsLong() == 1L),
                    "the next surviving log record must carry the drop count");
        } finally {
            messaging.release.countDown();
            logs.close();
        }
    }

    @Test
    void serviceConstructorInstallsNativeLogAppender() {
        LoggerContext context = (LoggerContext) LogManager.getContext(false);
        LogBusAppender.uninstall(context);
        MockConfigurationService config = config("{\"enabled\":true,\"minLevel\":\"INFO\"}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            org.apache.logging.log4j.Logger logger = LogManager.getLogger("edgecommons.capture.lifecycle");
            logger.info("captured without manual install");

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertFalse(messaging.getPublishedMessages().isEmpty());
            JsonObject body = publishedBodyForLogger(messaging, "edgecommons.capture.lifecycle");
            assertEquals("captured without manual install", body.get("message").getAsString());
        } finally {
            logs.close();
        }
    }

    @Test
    void nativeAppenderCapturesPreExistingApplicationLoggers() {
        LoggerContext context = (LoggerContext) LogManager.getContext(false);
        LogBusAppender.uninstall(context);
        MockConfigurationService config = config("{\"enabled\":true,\"minLevel\":\"INFO\"}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            PRECREATED_APP_LOGGER.info("[palletizer1] connected to opc.tcp://example:49320 (policy=None)");

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertFalse(messaging.getPublishedMessages().isEmpty());
            JsonObject body = publishedBodyForLogger(
                    messaging, "com.mbreissi.edgecommons.opcua.opc.OpcUaConnection");
            assertEquals("[palletizer1] connected to opc.tcp://example:49320 (policy=None)",
                    body.get("message").getAsString());
        } finally {
            logs.close();
        }
    }

    @Test
    void nativeAppenderCapturesApplicationLoggersAfterGeneratedConfigReconfigure(@TempDir Path dir) throws Exception {
        LoggerContext context = (LoggerContext) LogManager.getContext(false);
        LogBusAppender.uninstall(context);
        ConfigManager configManager = createConfigManager(dir, Platform.HOST, """
                {"level":"INFO","publish":{"enabled":true,"destination":"local","minLevel":"INFO"}}""",
                "test-thing");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(configManager, messaging);
        try {
            PRECREATED_APP_LOGGER.info("[palletizer1] connected to opc.tcp://example:49320 (policy=None)");

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertFalse(messaging.getPublishedMessages().isEmpty());
            JsonObject body = publishedBodyForLogger(
                    messaging, "com.mbreissi.edgecommons.opcua.opc.OpcUaConnection");
            assertEquals("[palletizer1] connected to opc.tcp://example:49320 (policy=None)",
                    body.get("message").getAsString());
        } finally {
            logs.close();
            Configurator.reconfigure();
        }
    }

    @Test
    void generatedConfigDoesNotInstallNativeAppenderWhenPublishDisabled(@TempDir Path dir) throws Exception {
        LoggerContext context = (LoggerContext) LogManager.getContext(false);
        LogBusAppender.uninstall(context);
        try {
            createConfigManager(dir, Platform.HOST, """
                    {"level":"INFO","publish":{"enabled":false,"captureNative":true}}""",
                    "test-thing");

            LoggerContext liveContext = (LoggerContext) LogManager.getContext(false);
            assertNull(liveContext.getConfiguration().getAppender(LogBusAppender.APPENDER_NAME));
        } finally {
            Configurator.reconfigure();
        }
    }

    @Test
    void nativeAppenderCapturesParameterizedApplicationLogsFromWorkerThread() throws Exception {
        LoggerContext context = (LoggerContext) LogManager.getContext(false);
        LogBusAppender.uninstall(context);
        MockConfigurationService config = config("{\"enabled\":true,\"minLevel\":\"INFO\"}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            Thread worker = new Thread(
                    () -> PRECREATED_APP_LOGGER.info("[{}] connected to {} (policy={})",
                            "palletizer1", "opc.tcp://example:49320", "None"),
                    "adapter-palletizer1");
            worker.start();
            worker.join(2_000);

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertFalse(messaging.getPublishedMessages().isEmpty());
            JsonObject body = publishedBodyForLogger(
                    messaging, "com.mbreissi.edgecommons.opcua.opc.OpcUaConnection");
            assertEquals("[palletizer1] connected to opc.tcp://example:49320 (policy=None)",
                    body.get("message").getAsString());
            assertEquals("adapter-palletizer1", body.get("thread").getAsString());
        } finally {
            logs.close();
        }
    }

    @Test
    void log4jAppenderCapturesNativeLogEvents() {
        MockConfigurationService config = config("{\"enabled\":true,\"minLevel\":\"INFO\"}");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            LoggerContext context = (LoggerContext) LogManager.getContext(false);
            LogBusAppender.install(context, true);
            org.apache.logging.log4j.Logger logger = LogManager.getLogger("edgecommons.capture.test");
            logger.info("captured appender message");

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertFalse(messaging.getPublishedMessages().isEmpty());
            JsonObject body = publishedBodyForLogger(messaging, "edgecommons.capture.test");
            assertEquals("captured appender message", body.get("message").getAsString());
        } finally {
            logs.close();
        }
    }

    @Test
    void captureNativeFalseSuppressesAppenderEvents() {
        MockConfigurationService config = config("""
                {"enabled":true,"captureNative":false,"minLevel":"INFO"}""");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            LoggerContext context = (LoggerContext) LogManager.getContext(false);
            LogBusAppender.install(context, true);
            org.apache.logging.log4j.Logger logger = LogManager.getLogger("edgecommons.capture.disabled");
            logger.info("not captured");

            assertTrue(logs.flush(Duration.ofMillis(250)));
            assertTrue(messaging.getPublishedMessages().isEmpty());
        } finally {
            logs.close();
        }
    }

    @Test
    void consoleCaptureParsesDefaultJavaLogPattern() {
        MockConfigurationService config = config("""
                {"enabled":true,"captureNative":false,"captureConsole":true,"minLevel":"INFO"}""");
        MockMessagingService messaging = new MockMessagingService();
        LogService logs = new LogService(config, messaging);
        try {
            System.out.println("2026-07-09 13:22:45.123 [INFO ] OpcUaConnection ( 137) [adapter-palletizer1] : [palletizer1] connected to opc.tcp://example:49320 (policy=None)");

            assertTrue(logs.flush(Duration.ofSeconds(2)));
            assertFalse(messaging.getPublishedMessages().isEmpty());
            JsonObject body = publishedBodyForLogger(messaging, "OpcUaConnection");
            assertEquals("INFO", body.get("level").getAsString());
            assertEquals("adapter-palletizer1", body.get("thread").getAsString());
            assertEquals("[palletizer1] connected to opc.tcp://example:49320 (policy=None)",
                    body.get("message").getAsString());
        } finally {
            logs.close();
        }
    }

    private static ConfigManager createConfigManager(Path dir, Platform platform, String loggingBody, String thing)
            throws IOException {
        File cfgFile = File.createTempFile("log-service", ".json", dir.toFile());
        try (FileWriter w = new FileWriter(cfgFile)) {
            w.write("{"
                    + "\"logging\":" + loggingBody + ","
                    + "\"metricEmission\":{\"target\":\"log\"},"
                    + "\"heartbeat\":{\"intervalSecs\":30},"
                    + "\"tags\":{},"
                    + "\"component\":{\"global\":{}}"
                    + "}");
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

    private static final class BlockingMessaging extends MockMessagingService {
        final CountDownLatch started = new CountDownLatch(1);
        final CountDownLatch release = new CountDownLatch(1);

        @Override
        protected void publishReserved(String topic, Message message) {
            if (started.getCount() > 0) {
                started.countDown();
                try {
                    release.await(2, TimeUnit.SECONDS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            }
            super.publishReserved(topic, message);
        }
    }
}
