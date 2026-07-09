package com.mbreissi.edgecommons.logging;

import org.apache.logging.log4j.Level;

/**
 * The EdgeCommons log bus levels, serialized on the wire as uppercase strings and in topics as
 * lowercase channel tokens.
 */
public enum LogLevel {
    TRACE,
    DEBUG,
    INFO,
    WARN,
    ERROR,
    FATAL;

    /** Parses a level name case-insensitively. */
    public static LogLevel parse(String value) {
        if (value == null || value.isBlank()) {
            throw new IllegalArgumentException("log level must be non-empty");
        }
        return LogLevel.valueOf(value.trim().toUpperCase());
    }

    /** Converts a Log4j2 level into the closest log bus level. */
    public static LogLevel fromLog4j(Level level) {
        if (level == null) {
            return INFO;
        }
        if (level.isMoreSpecificThan(Level.FATAL)) {
            return FATAL;
        }
        if (level.isMoreSpecificThan(Level.ERROR)) {
            return ERROR;
        }
        if (level.isMoreSpecificThan(Level.WARN)) {
            return WARN;
        }
        if (level.isMoreSpecificThan(Level.INFO)) {
            return INFO;
        }
        if (level.isMoreSpecificThan(Level.DEBUG)) {
            return DEBUG;
        }
        return TRACE;
    }

    /** Returns the topic channel token for this level. */
    public String topicToken() {
        return name().toLowerCase();
    }
}
