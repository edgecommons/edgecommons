//! # Credential access audit
//!
//! **One-liner purpose**: Emit non-sensitive credential-access events (operation, secret name,
//! version, source, outcome — **never the value**) to a pluggable [`AuditSink`].
//!
//! ## Overview
//! A secrets subsystem should record who-touched-what-when. [`DefaultCredentialService`] emits an
//! [`AuditEvent`] on each value-touching/mutating op (`get`/`get_version`/`put`/`delete`) when an
//! audit sink is configured (`credentials.audit.enabled`). The default [`LogAuditSink`] writes a
//! structured `tracing` record on a dedicated target so the audit trail can be filtered/routed
//! independently; a custom [`AuditSink`] can forward events to any log/metric/SIEM pipeline.
//!
//! ## Safety
//! Events carry only metadata — the secret value is never included. Sinks are called inline on the
//! credential path (after the vault lock is released), so implementations must be cheap and
//! non-blocking.
//!
//! [`DefaultCredentialService`]: super::service::DefaultCredentialService

use std::sync::Arc;

/// The `tracing` target the default sink emits on (filter/route the audit trail independently).
pub const AUDIT_TARGET: &str = "edgecommons::credentials::audit";

/// A single credential-access audit event. **Never contains the secret value.**
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// Operation: `"get"` | `"put"` | `"delete"`.
    pub op: &'static str,
    /// Caller-facing secret name (transparent namespace stripped).
    pub name: String,
    /// Version touched, or `"-"` when not applicable / not found.
    pub version: String,
    /// Origin of the value: `"local"` | `"central"` | `"-"`.
    pub source: String,
    /// Result: `"hit"` | `"miss"` | `"ok"`.
    pub outcome: &'static str,
}

/// Destination for audit events. Must be `Send + Sync`, cheap, non-blocking, and must never log the
/// secret value.
pub trait AuditSink: Send + Sync {
    /// Record one access event.
    fn record(&self, event: &AuditEvent);
}

/// Default sink: emit each event as a structured `tracing` record on [`AUDIT_TARGET`].
pub struct LogAuditSink;

impl AuditSink for LogAuditSink {
    fn record(&self, e: &AuditEvent) {
        tracing::info!(
            target: AUDIT_TARGET,
            op = e.op,
            secret = %e.name,
            version = %e.version,
            source = %e.source,
            outcome = e.outcome,
            "credential access",
        );
    }
}

/// The default logging audit sink as a trait object (used when `credentials.audit.enabled`).
pub fn log_sink() -> Arc<dyn AuditSink> {
    Arc::new(LogAuditSink)
}
