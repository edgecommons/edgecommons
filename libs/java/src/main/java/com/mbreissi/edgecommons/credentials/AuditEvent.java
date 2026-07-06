package com.mbreissi.edgecommons.credentials;

/**
 * A single credential-access audit event. <strong>Never contains the secret value.</strong>
 *
 * <p>Carries only metadata: the operation, the caller-facing secret name (transparent namespace
 * stripped), the version touched, the value's origin, and the outcome.
 *
 * @param op      operation: {@code "get"} | {@code "put"} | {@code "delete"}
 * @param name    caller-facing secret name (transparent namespace stripped)
 * @param version version touched, or {@code "-"} when not applicable / not found
 * @param source  origin of the value: {@code "local"} | {@code "central"} | {@code "-"}
 * @param outcome result: {@code "hit"} | {@code "miss"} | {@code "ok"}
 */
public record AuditEvent(String op, String name, String version, String source, String outcome) {
}
