package com.mbreissi.edgecommons.logging;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationChangeListener;
import com.mbreissi.edgecommons.config.LoggingConfiguration.LogPublishConfiguration;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageBuilder;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.messaging.MessagingClient;
import com.mbreissi.edgecommons.messaging.Qos;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;

import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicLong;
import java.util.regex.Pattern;
import java.util.regex.PatternSyntaxException;

/**
 * Publishes structured log records on {@code ecv1/{device}/{component}/main/log/{level}} through
 * the library-owned reserved UNS publishing seam.
 */
public final class LogService implements ConfigurationChangeListener, AutoCloseable {
    private static final Gson GSON = new GsonBuilder().serializeNulls().create();
    private static final ThreadLocal<Boolean> PUBLISHING = ThreadLocal.withInitial(() -> false);

    private final ConfigManager configManager;
    private final MessagingClient messagingClient;
    private final Object lock = new Object();
    private final ArrayDeque<LogRecord> queue = new ArrayDeque<>();
    private final AtomicLong sequence = new AtomicLong();
    private final AtomicLong enqueuedRecords = new AtomicLong();
    private final AtomicLong publishedRecords = new AtomicLong();
    private final AtomicLong droppedRecords = new AtomicLong();
    private final AtomicLong droppedSinceLastPublish = new AtomicLong();
    private final AtomicLong filteredRecords = new AtomicLong();
    private final AtomicLong redactedRecords = new AtomicLong();
    private final AtomicLong truncatedRecords = new AtomicLong();
    private final AtomicLong publishFailures = new AtomicLong();

    private volatile RuntimeConfig runtimeConfig;
    private volatile boolean closed;
    private int processing;
    private final Thread worker;

    public LogService(ConfigManager configManager, MessagingClient messagingClient) {
        this.configManager = Objects.requireNonNull(configManager, "configManager must not be null");
        this.messagingClient = Objects.requireNonNull(messagingClient, "messagingClient must not be null");
        this.runtimeConfig = RuntimeConfig.from(configManager.getLoggingConfig().getPublishConfig());
        LogBusCapture.setService(this);
        configureNativeCapture(runtimeConfig);
        ConsoleCapture.configure(this, runtimeConfig.captureConsole);
        this.worker = new Thread(this::runWorker, "edgecommons-log-publisher");
        this.worker.setDaemon(true);
        this.worker.start();
    }

    /**
     * Queues a record for log-bus publication. The call never blocks application logging threads:
     * when the queue is full the oldest queued record is dropped.
     */
    public void publish(LogRecord record) {
        Objects.requireNonNull(record, "record must not be null");
        RuntimeConfig cfg = runtimeConfig;
        if (!cfg.enabled || record.getLevel().ordinal() < cfg.minLevel.ordinal()) {
            filteredRecords.incrementAndGet();
            return;
        }
        enqueue(assignSequence(record));
    }

    /** Waits until all records queued at the time of the call have been processed or the timeout expires. */
    public boolean flush(Duration timeout) {
        long timeoutNanos = timeout == null ? 0L : Math.max(0L, timeout.toNanos());
        long deadline = System.nanoTime() + timeoutNanos;
        synchronized (lock) {
            while (!(queue.isEmpty() && processing == 0)) {
                if (timeoutNanos == 0L) {
                    return false;
                }
                long remaining = deadline - System.nanoTime();
                if (remaining <= 0L) {
                    return false;
                }
                try {
                    lock.wait(Math.max(1L, remaining / 1_000_000L));
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    return false;
                }
            }
            return true;
        }
    }

    /** Returns a point-in-time stats snapshot. */
    public LogStats stats() {
        synchronized (lock) {
            return new LogStats(enqueuedRecords.get(), publishedRecords.get(), droppedRecords.get(),
                    filteredRecords.get(), redactedRecords.get(), truncatedRecords.get(),
                    publishFailures.get(), queue.size() + processing);
        }
    }

    @Override
    public boolean onConfigurationChanged() {
        RuntimeConfig cfg = RuntimeConfig.from(configManager.getLoggingConfig().getPublishConfig());
        runtimeConfig = cfg;
        configureNativeCapture(cfg);
        ConsoleCapture.configure(this, cfg.captureConsole);
        synchronized (lock) {
            while (queue.size() > cfg.maxRecords) {
                queue.removeFirst();
                recordDrop();
            }
            lock.notifyAll();
        }
        return true;
    }

    @Override
    public void close() {
        closed = true;
        LogBusAppender.uninstallAll();
        ConsoleCapture.configure(this, false);
        synchronized (lock) {
            lock.notifyAll();
        }
        LogBusCapture.clearService(this);
    }

    void captureNative(LogRecord record) {
        if (Boolean.TRUE.equals(PUBLISHING.get())) {
            return;
        }
        if (!runtimeConfig.captureNative) {
            return;
        }
        publish(record);
    }

    void captureConsole(LogRecord record) {
        if (Boolean.TRUE.equals(PUBLISHING.get()) || !runtimeConfig.captureConsole) {
            return;
        }
        publish(record);
    }

    static boolean isPublishingThread() {
        return Boolean.TRUE.equals(PUBLISHING.get());
    }

    private static void configureNativeCapture(RuntimeConfig cfg) {
        LogBusAppender.installAll(cfg.enabled && cfg.captureNative);
    }

    private LogRecord assignSequence(LogRecord record) {
        if (record.getSequence() != null) {
            return record;
        }
        return record.toBuilder().withSequence(sequence.incrementAndGet()).build();
    }

    private void enqueue(LogRecord record) {
        RuntimeConfig cfg = runtimeConfig;
        synchronized (lock) {
            if (closed) {
                return;
            }
            if (queue.size() >= cfg.maxRecords) {
                queue.removeFirst();
                recordDrop();
            }
            queue.addLast(record);
            enqueuedRecords.incrementAndGet();
            lock.notifyAll();
        }
    }

    private void recordDrop() {
        droppedRecords.incrementAndGet();
        droppedSinceLastPublish.incrementAndGet();
    }

    private void runWorker() {
        while (!closed) {
            LogRecord record;
            synchronized (lock) {
                while (!closed && queue.isEmpty()) {
                    try {
                        lock.wait();
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        return;
                    }
                }
                if (closed && queue.isEmpty()) {
                    return;
                }
                record = queue.removeFirst();
                processing++;
            }
            try {
                publishNow(record);
            } finally {
                synchronized (lock) {
                    processing--;
                    lock.notifyAll();
                }
            }
        }
    }

    private void publishNow(LogRecord original) {
        RuntimeConfig cfg = runtimeConfig;
        if (!cfg.enabled) {
            filteredRecords.incrementAndGet();
            return;
        }
        long dropped = droppedSinceLastPublish.getAndSet(0L);
        LogRecord record = dropped > 0 && original.getDropped() == null
                ? original.toBuilder().withDropped(dropped).build()
                : original;
        PreparedRecord prepared = prepare(record, cfg);
        MessageIdentity identity = configManager.getComponentIdentity();
        if (identity == null) {
            publishFailures.incrementAndGet();
            return;
        }
        if (!messagingClient.connected()) {
            publishFailures.incrementAndGet();
            return;
        }
        String topic = new Uns(identity, configManager.isTopicIncludeRoot())
                .topic(UnsClass.LOG, prepared.level().topicToken());
        Message message = MessageBuilder.create("log", "1.0")
                .withTimestamp(prepared.timestamp().toString())
                .withPayload(prepared.body())
                .withConfig(configManager)   // D‑U28: log is component scope (no instance)
                .build();
        try {
            PUBLISHING.set(true);
            if (cfg.destination == LogPublishConfiguration.Destination.NORTHBOUND) {
                messagingClient.reservedPublisher().publishNorthbound(topic, message, Qos.AT_LEAST_ONCE);
            } else {
                messagingClient.reservedPublisher().publish(topic, message);
            }
            publishedRecords.incrementAndGet();
        } catch (RuntimeException e) {
            publishFailures.incrementAndGet();
        } finally {
            PUBLISHING.set(false);
        }
    }

    private PreparedRecord prepare(LogRecord record, RuntimeConfig cfg) {
        JsonObject body = body(record);
        boolean redacted = false;
        if (cfg.redactionEnabled) {
            JsonElement redactedBody = redact(body, cfg);
            redacted = !redactedBody.equals(body);
            body = redactedBody.getAsJsonObject();
        }
        boolean truncated = false;
        if (utf8Bytes(body) > cfg.maxRecordBytes) {
            truncated = true;
            body.addProperty("truncated", true);
            truncateStringField(body, "message", cfg.maxRecordBytes);
            if (utf8Bytes(body) > cfg.maxRecordBytes) {
                shrinkErrorField(body, cfg.maxRecordBytes);
            }
            if (utf8Bytes(body) > cfg.maxRecordBytes) {
                body.remove("fields");
            }
            if (utf8Bytes(body) > cfg.maxRecordBytes) {
                body.remove("error");
            }
        }
        if (redacted) {
            redactedRecords.incrementAndGet();
        }
        if (truncated) {
            truncatedRecords.incrementAndGet();
        }
        return new PreparedRecord(record.getTimestamp(), record.getLevel(), body);
    }

    private static JsonObject body(LogRecord record) {
        JsonObject body = new JsonObject();
        body.addProperty("schema", "edgecommons.log.v1");
        body.addProperty("timestamp", record.getTimestamp().toString());
        body.addProperty("level", record.getLevel().name());
        body.addProperty("logger", record.getLogger());
        body.addProperty("message", record.getMessage());
        body.addProperty("sequence", record.getSequence());
        if (record.getThread() != null) body.addProperty("thread", record.getThread());
        if (record.getFields() != null) body.add("fields", GSON.toJsonTree(record.getFields()));
        if (record.getError() != null) body.add("error", record.getError());
        if (record.getTruncated() != null) body.addProperty("truncated", record.getTruncated());
        if (record.getDropped() != null && record.getDropped() > 0) {
            body.addProperty("dropped", record.getDropped());
        }
        return body;
    }

    private static JsonElement redact(JsonElement element, RuntimeConfig cfg) {
        if (element == null || element.isJsonNull()) {
            return element;
        }
        if (element.isJsonPrimitive() && element.getAsJsonPrimitive().isString()) {
            String value = element.getAsString();
            String redacted = value;
            for (Pattern pattern : cfg.redactionPatterns) {
                redacted = pattern.matcher(redacted).replaceAll(cfg.redactionReplacement);
            }
            return GSON.toJsonTree(redacted);
        }
        if (element.isJsonArray()) {
            com.google.gson.JsonArray copy = new com.google.gson.JsonArray();
            for (JsonElement child : element.getAsJsonArray()) {
                copy.add(redact(child, cfg));
            }
            return copy;
        }
        if (element.isJsonObject()) {
            JsonObject copy = new JsonObject();
            for (String key : element.getAsJsonObject().keySet()) {
                copy.add(key, redact(element.getAsJsonObject().get(key), cfg));
            }
            return copy;
        }
        return element;
    }

    private static void truncateStringField(JsonObject body, String field, int maxBytes) {
        truncateStringField(body, body, field, maxBytes);
    }

    private static void truncateStringField(JsonObject fullBody, JsonObject owner, String field, int maxBytes) {
        if (!owner.has(field) || !owner.get(field).isJsonPrimitive()
                || !owner.get(field).getAsJsonPrimitive().isString()) {
            return;
        }
        String value = owner.get(field).getAsString();
        while (!value.isEmpty() && utf8Bytes(fullBody) > maxBytes) {
            int nextLength = Math.max(0, value.length() - Math.max(1, (utf8Bytes(fullBody) - maxBytes) / 2));
            value = value.substring(0, nextLength);
            owner.addProperty(field, value);
        }
    }

    private static void shrinkErrorField(JsonObject body, int maxBytes) {
        if (!body.has("error")) {
            return;
        }
        JsonElement error = body.get("error");
        if (error.isJsonPrimitive() && error.getAsJsonPrimitive().isString()) {
            truncateStringField(body, "error", maxBytes);
            return;
        }
        if (!error.isJsonObject()) {
            body.remove("error");
            return;
        }
        JsonObject errorObject = error.getAsJsonObject();
        truncateStringField(body, errorObject, "stack", maxBytes);
        if (utf8Bytes(body) > maxBytes) {
            errorObject.remove("stack");
        }
        if (utf8Bytes(body) > maxBytes) {
            truncateStringField(body, errorObject, "message", maxBytes);
        }
        if (utf8Bytes(body) > maxBytes) {
            JsonObject slim = new JsonObject();
            if (errorObject.has("type")) {
                slim.add("type", errorObject.get("type"));
            }
            if (errorObject.has("message")) {
                slim.add("message", errorObject.get("message"));
            }
            slim.addProperty("truncated", true);
            body.add("error", slim);
            truncateStringField(body, slim, "message", maxBytes);
        }
    }

    private static int utf8Bytes(JsonObject body) {
        return GSON.toJson(body).getBytes(StandardCharsets.UTF_8).length;
    }

    private record PreparedRecord(Instant timestamp, LogLevel level, JsonObject body) {}

    private static final class RuntimeConfig {
        private final boolean enabled;
        private final LogPublishConfiguration.Destination destination;
        private final LogLevel minLevel;
        private final boolean captureNative;
        private final boolean captureConsole;
        private final int maxRecordBytes;
        private final int maxRecords;
        private final boolean redactionEnabled;
        private final String redactionReplacement;
        private final List<Pattern> redactionPatterns;

        private RuntimeConfig(LogPublishConfiguration cfg) {
            this.enabled = cfg.isEnabled();
            this.destination = cfg.getDestination();
            this.minLevel = LogLevel.parse(cfg.getMinLevel());
            this.captureNative = cfg.isCaptureNative();
            this.captureConsole = cfg.isCaptureConsole();
            this.maxRecordBytes = cfg.getMaxRecordBytes();
            this.maxRecords = cfg.getQueueMaxRecords();
            this.redactionEnabled = cfg.isRedactionEnabled();
            this.redactionReplacement = cfg.getRedactionReplacement();
            this.redactionPatterns = compilePatterns(cfg.getRedactionExtraPatterns());
        }

        static RuntimeConfig from(LogPublishConfiguration cfg) {
            return new RuntimeConfig(cfg);
        }

        private static List<Pattern> compilePatterns(List<String> extraPatterns) {
            List<Pattern> patterns = new ArrayList<>();
            patterns.add(Pattern.compile("(?i)(password|passwd|pwd|secret|token|api[_-]?key)\\s*[:=]\\s*\\S+"));
            for (String pattern : extraPatterns) {
                try {
                    patterns.add(Pattern.compile(pattern));
                } catch (PatternSyntaxException e) {
                    throw new IllegalArgumentException(
                            "Invalid logging.publish.redaction.extraPatterns regex: " + pattern, e);
                }
            }
            return List.copyOf(patterns);
        }
    }
}
