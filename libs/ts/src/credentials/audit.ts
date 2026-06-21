/**
 * Credential access audit — emit non-sensitive access events to a pluggable sink.
 *
 * **One-liner purpose**: Record who-touched-what-when for the secrets subsystem
 * (operation, secret name, version, source, outcome — **never the value**) to a
 * pluggable {@link AuditSink}.
 *
 * Mirrors the Rust `credentials/audit.rs`: {@link DefaultCredentialService} emits
 * an {@link AuditEvent} on each value-touching/mutating op (`get`/`getVersion`/
 * `put`/`delete`) when an audit sink is configured (`credentials.audit.enabled`).
 * The default {@link LogAuditSink} writes a structured line via the library logger
 * so the audit trail can be filtered/routed; a custom {@link AuditSink} can forward
 * events to any log/metric/SIEM pipeline.
 *
 * ## Safety
 * Events carry only metadata — the secret value is never included. Sinks are called
 * inline on the credential path, so implementations must be cheap and non-blocking.
 */
import { logger } from "../logging";

/** A single credential-access audit event. **Never contains the secret value.** */
export interface AuditEvent {
  /** Operation: `"get"` | `"put"` | `"delete"`. */
  op: string;
  /** Caller-facing secret name (transparent namespace stripped). */
  name: string;
  /** Version touched, or `"-"` when not applicable / not found. */
  version: string;
  /** Origin of the value: `"local"` | `"central"` | `"-"`. */
  source: string;
  /** Result: `"hit"` | `"miss"` | `"ok"`. */
  outcome: string;
}

/**
 * Destination for audit events. Must be cheap, non-blocking, and must never log
 * the secret value.
 */
export interface AuditSink {
  /** Record one access event. */
  record(event: AuditEvent): void;
}

/** Default sink: emit each event as a structured line via the library logger. */
export class LogAuditSink implements AuditSink {
  record(e: AuditEvent): void {
    logger.info(
      `credential access op=${e.op} secret=${e.name} version=${e.version} source=${e.source} outcome=${e.outcome}`,
    );
  }
}

/** The default logging audit sink (used when `credentials.audit.enabled`). */
export function logSink(): AuditSink {
  return new LogAuditSink();
}
