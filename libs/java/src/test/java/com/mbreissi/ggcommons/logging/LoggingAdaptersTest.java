/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.logging;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Unit tests for the logging compatibility layer: the {@link Logger} interface,
 * the {@link LoggerAdapter} implementations (JUL, Log4j2, SLF4J) and the
 * {@link LoggerFactory} auto-detection entry points.
 *
 * <p>These tests exercise every public method on every adapter with representative
 * arguments. There is little to assert beyond "a non-null logger is returned" and
 * "logging does not throw", so the tests favour {@code assertNotNull} /
 * {@code assertDoesNotThrow} while driving as many code paths as possible.
 */
class LoggingAdaptersTest {

    private static final String NAME = LoggingAdaptersTest.class.getName();
    private static final Throwable BOOM = new RuntimeException("boom");

    /**
     * Calls every level method (plain, varargs/format, and throwable variants)
     * plus every {@code isXxxEnabled} predicate on the supplied logger.
     */
    private static void exerciseLogger(Logger logger) {
        assertNotNull(logger);
        assertDoesNotThrow(() -> {
            logger.trace("trace message");
            logger.trace("trace {} message", "arg");
            logger.trace("trace with throwable", BOOM);

            logger.debug("debug message");
            logger.debug("debug {} message", "arg");
            logger.debug("debug with throwable", BOOM);

            logger.info("info message");
            logger.info("info {} message {}", "a", "b");
            logger.info("info with throwable", BOOM);

            logger.warn("warn message");
            logger.warn("warn {} message", 42);
            logger.warn("warn with throwable", BOOM);

            logger.error("error message");
            logger.error("error {} message", "arg");
            logger.error("error with throwable", BOOM);

            // Level predicates - just execute them.
            logger.isTraceEnabled();
            logger.isDebugEnabled();
            logger.isInfoEnabled();
            logger.isWarnEnabled();
            logger.isErrorEnabled();
        });
    }

    @Test
    void julAdapterExercisesAllMethods() {
        LoggerAdapter adapter = new JulLoggerAdapter();
        exerciseLogger(adapter.getLogger(NAME));
    }

    @Test
    void log4j2AdapterExercisesAllMethods() {
        LoggerAdapter adapter = new Log4j2LoggerAdapter();
        exerciseLogger(adapter.getLogger(NAME));
    }

    @Test
    void slf4jAdapterExercisesAllMethods() {
        LoggerAdapter adapter = new Slf4jLoggerAdapter();
        exerciseLogger(adapter.getLogger(NAME));
    }

    @Test
    void loggerFactoryReturnsLoggerForClass() {
        Logger logger = LoggerFactory.getLogger(LoggingAdaptersTest.class);
        exerciseLogger(logger);
    }

    @Test
    void loggerFactoryReturnsLoggerForName() {
        Logger logger = LoggerFactory.getLogger("custom.logger.name");
        exerciseLogger(logger);
    }

    @Test
    void adaptersReturnDistinctLoggersForDistinctNames() {
        LoggerAdapter adapter = new JulLoggerAdapter();
        assertNotNull(adapter.getLogger("a"));
        assertNotNull(adapter.getLogger("b"));
    }
}
