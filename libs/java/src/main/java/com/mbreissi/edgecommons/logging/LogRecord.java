package com.mbreissi.edgecommons.logging;

import com.google.gson.JsonElement;

import java.time.Instant;
import java.util.LinkedHashMap;
import java.util.Map;

/** A structured record published on the EdgeCommons UNS {@code log} class. */
public final class LogRecord {
    private final Instant timestamp;
    private final LogLevel level;
    private final String logger;
    private final String message;
    private final Long sequence;
    private final String thread;
    private final Map<String, Object> fields;
    private final JsonElement error;
    private final Boolean truncated;
    private final Long dropped;

    private LogRecord(Builder builder) {
        this.timestamp = builder.timestamp == null ? Instant.now() : builder.timestamp;
        this.level = builder.level == null ? LogLevel.INFO : builder.level;
        this.logger = requireNonBlank(builder.logger, "logger");
        this.message = builder.message == null ? "" : builder.message;
        this.sequence = builder.sequence;
        this.thread = blankToNull(builder.thread);
        this.fields = builder.fields == null || builder.fields.isEmpty()
                ? null
                : Map.copyOf(builder.fields);
        this.error = builder.error;
        this.truncated = builder.truncated;
        this.dropped = builder.dropped;
    }

    public static Builder builder() {
        return new Builder();
    }

    Builder toBuilder() {
        Builder builder = builder()
                .withTimestamp(timestamp)
                .withLevel(level)
                .withLogger(logger)
                .withMessage(message);
        if (sequence != null) builder.withSequence(sequence);
        if (thread != null) builder.withThread(thread);
        if (fields != null) builder.withFields(fields);
        if (error != null) builder.withError(error);
        if (truncated != null) builder.withTruncated(truncated);
        if (dropped != null) builder.withDropped(dropped);
        return builder;
    }

    public Instant getTimestamp() { return timestamp; }
    public LogLevel getLevel() { return level; }
    public String getLogger() { return logger; }
    public String getMessage() { return message; }
    public Long getSequence() { return sequence; }
    public String getThread() { return thread; }
    public Map<String, Object> getFields() { return fields; }
    public JsonElement getError() { return error; }
    public Boolean getTruncated() { return truncated; }
    public Long getDropped() { return dropped; }

    private static String requireNonBlank(String value, String name) {
        if (value == null || value.isBlank()) {
            throw new IllegalArgumentException(name + " must be non-empty");
        }
        return value;
    }

    private static String blankToNull(String value) {
        return value == null || value.isBlank() ? null : value;
    }

    /** Fluent builder for {@link LogRecord}. */
    public static final class Builder {
        private Instant timestamp;
        private LogLevel level;
        private String logger;
        private String message;
        private Long sequence;
        private String thread;
        private Map<String, Object> fields;
        private JsonElement error;
        private Boolean truncated;
        private Long dropped;

        private Builder() {}

        public Builder withTimestamp(Instant timestamp) {
            this.timestamp = timestamp;
            return this;
        }

        public Builder withLevel(LogLevel level) {
            this.level = level;
            return this;
        }

        public Builder withLevel(String level) {
            this.level = LogLevel.parse(level);
            return this;
        }

        public Builder withLogger(String logger) {
            this.logger = logger;
            return this;
        }

        public Builder withMessage(String message) {
            this.message = message;
            return this;
        }

        public Builder withSequence(long sequence) {
            if (sequence < 0) {
                throw new IllegalArgumentException("sequence must be non-negative");
            }
            this.sequence = sequence;
            return this;
        }

        public Builder withThread(String thread) {
            this.thread = thread;
            return this;
        }

        public Builder withFields(Map<String, ?> fields) {
            if (fields == null || fields.isEmpty()) {
                this.fields = null;
            } else {
                this.fields = new LinkedHashMap<>();
                for (Map.Entry<String, ?> entry : fields.entrySet()) {
                    if (entry.getKey() != null) {
                        this.fields.put(entry.getKey(), entry.getValue());
                    }
                }
            }
            return this;
        }

        public Builder addField(String key, Object value) {
            if (key == null) {
                return this;
            }
            if (fields == null) {
                fields = new LinkedHashMap<>();
            }
            fields.put(key, value);
            return this;
        }

        public Builder withError(JsonElement error) {
            this.error = error;
            return this;
        }

        public Builder withTruncated(boolean truncated) {
            this.truncated = truncated;
            return this;
        }

        public Builder withDropped(long dropped) {
            if (dropped < 0) {
                throw new IllegalArgumentException("dropped must be non-negative");
            }
            this.dropped = dropped;
            return this;
        }

        public LogRecord build() {
            return new LogRecord(this);
        }
    }
}
