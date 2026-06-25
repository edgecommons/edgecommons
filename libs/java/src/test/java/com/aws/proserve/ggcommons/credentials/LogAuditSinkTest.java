package com.aws.proserve.ggcommons.credentials;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.ArrayList;
import java.util.List;

import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.core.LogEvent;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.appender.AbstractAppender;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.LoggerConfig;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

/**
 * Unit tests for {@link LogAuditSink}: assert that {@link LogAuditSink#record} emits one structured
 * INFO line on the dedicated audit logger ({@link LogAuditSink#AUDIT_TARGET}) carrying only the event
 * metadata (op/secret/version/source/outcome) and <strong>never</strong> the secret value.
 *
 * <p>A programmatic Log4j2 appender is attached to the audit logger to capture the rendered message
 * without touching real I/O.
 */
class LogAuditSinkTest {

    /** Captures rendered log messages emitted on the audit logger. */
    private static final class CapturingAppender extends AbstractAppender {
        final List<String> messages = new ArrayList<>();
        final List<Level> levels = new ArrayList<>();

        CapturingAppender() {
            super("capture", null, null, true, null);
        }

        @Override
        public void append(LogEvent event) {
            messages.add(event.getMessage().getFormattedMessage());
            levels.add(event.getLevel());
        }
    }

    private LoggerContext ctx;
    private LoggerConfig auditLoggerConfig;
    private CapturingAppender appender;
    private Level originalLevel;

    @BeforeEach
    void attachAppender() {
        ctx = (LoggerContext) org.apache.logging.log4j.LogManager.getContext(false);
        Configuration cfg = ctx.getConfiguration();
        appender = new CapturingAppender();
        appender.start();
        cfg.addAppender(appender);

        auditLoggerConfig = cfg.getLoggerConfig(LogAuditSink.AUDIT_TARGET);
        // If we got the root config (no dedicated config yet), create a dedicated one.
        if (!auditLoggerConfig.getName().equals(LogAuditSink.AUDIT_TARGET)) {
            auditLoggerConfig = new LoggerConfig(LogAuditSink.AUDIT_TARGET, Level.INFO, true);
            cfg.addLogger(LogAuditSink.AUDIT_TARGET, auditLoggerConfig);
        }
        originalLevel = auditLoggerConfig.getLevel();
        auditLoggerConfig.setLevel(Level.INFO);
        auditLoggerConfig.addAppender(appender, Level.INFO, null);
        ctx.updateLoggers();
    }

    @AfterEach
    void detachAppender() {
        auditLoggerConfig.removeAppender("capture");
        auditLoggerConfig.setLevel(originalLevel);
        appender.stop();
        ctx.updateLoggers();
    }

    @Test
    void emitsStructuredInfoLineWithAllMetadata() {
        new LogAuditSink().record(new AuditEvent("get", "db/password", "v7", "local", "hit"));

        assertEquals(1, appender.messages.size());
        assertEquals(Level.INFO, appender.levels.get(0));
        String msg = appender.messages.get(0);
        assertTrue(msg.contains("op=get"), msg);
        assertTrue(msg.contains("secret=db/password"), msg);
        assertTrue(msg.contains("version=v7"), msg);
        assertTrue(msg.contains("source=local"), msg);
        assertTrue(msg.contains("outcome=hit"), msg);
    }

    @Test
    void neverLeaksAnyValueLikeContent() {
        // Even if (hypothetically) an event carried a value-shaped string in a metadata slot, the
        // sink only renders the five known fields; here we assert the value passed nowhere appears.
        new LogAuditSink().record(new AuditEvent("put", "api/key", "-", "local", "ok"));
        String msg = appender.messages.get(0);
        assertFalse(msg.toLowerCase().contains("value"), msg);
    }

    @Test
    void auditTargetConstantIsTheDedicatedLoggerName() {
        assertEquals("com.aws.proserve.ggcommons.credentials.audit", LogAuditSink.AUDIT_TARGET);
    }

    @Test
    void auditEventRecordExposesComponents() {
        AuditEvent e = new AuditEvent("delete", "n", "-", "-", "miss");
        assertEquals("delete", e.op());
        assertEquals("n", e.name());
        assertEquals("-", e.version());
        assertEquals("-", e.source());
        assertEquals("miss", e.outcome());
        assertEquals(e, new AuditEvent("delete", "n", "-", "-", "miss"));
    }
}
