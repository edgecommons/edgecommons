package com.mbreissi.ggcommons.credentials;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Default {@link AuditSink}: emit each event as a structured line on a dedicated logger so the audit
 * trail can be filtered/routed independently of the rest of the credentials logging.
 *
 * <p>Logs only metadata (op/secret/version/source/outcome) at INFO. <strong>Never the value.</strong>
 */
public final class LogAuditSink implements AuditSink {
    /** The dedicated logger name the default sink emits on (filter/route the audit trail). */
    public static final String AUDIT_TARGET = "com.mbreissi.ggcommons.credentials.audit";

    private static final Logger LOGGER = LogManager.getLogger(AUDIT_TARGET);

    @Override
    public void record(AuditEvent e) {
        LOGGER.info("credential access op={} secret={} version={} source={} outcome={}",
                e.op(), e.name(), e.version(), e.source(), e.outcome());
    }
}
