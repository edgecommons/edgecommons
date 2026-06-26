package com.mbreissi.ggcommons.credentials;

/**
 * Destination for credential-access audit events. Implementations must be cheap, non-blocking, and
 * must never log the secret value (the {@link AuditEvent} carries only metadata).
 *
 * <p>Sinks are called inline on the credential path (after the vault lock is released).
 */
public interface AuditSink {
    /** Record one access event. */
    void record(AuditEvent event);
}
