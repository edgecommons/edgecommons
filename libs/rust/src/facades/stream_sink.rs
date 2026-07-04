//! # StreamSink — the seam `data()` composes for a `stream:<name>` channel
//!
//! **One-liner purpose**: The decoupling seam [`crate::facades::DataFacade`] composes to route a
//! `stream:<name>` channel into the telemetry streaming service (DESIGN-class-facades §4: "the
//! facade *composes* `StreamService`, it does not replace it"), mirroring the Java canonical
//! `com.mbreissi.ggcommons.facades.StreamSink`.
//!
//! Kept independent of [`crate::streaming`] (and thus buildable without the `streaming` cargo
//! feature) so `DataFacade` never needs to depend on the native `ggstreamlog` binding directly —
//! production wires it to the real stream service (`streaming::StreamServiceSink`, feature-gated);
//! tests inject a recording fake. When no sink is wired (`None` on the owning
//! [`crate::GgInstance`] — either no `streaming` section is configured, or the `streaming` cargo
//! feature is off), a `stream:` route falls back to a LOCAL publish (readiness / no-streaming →
//! local, D1a) rather than dropping the record.

use crate::error::Result;

/// Appends one durable record to a named stream — the `data()` facade's stream-route seam.
pub trait StreamSink: Send + Sync {
    /// Appends one durable record.
    ///
    /// # Parameters
    /// - `stream_name` — the configured stream name (the `stream:<name>` target).
    /// - `partition_key` — the routing/ordering key — the signal's stable `signal.id`.
    /// - `timestamp_ms` — the producer timestamp (epoch millis, from the sample's `serverTs`).
    /// - `payload` — the serialized envelope bytes (the exact bytes a bus publish would carry).
    ///
    /// # Errors
    /// Implementation-defined (e.g. an unconfigured stream name, or a durable-log failure); the
    /// facade catches and logs any error — a stream/northbound transport failure must never flip
    /// local readiness (DESIGN-class-facades §4).
    fn append(
        &self,
        stream_name: &str,
        partition_key: &str,
        timestamp_ms: u64,
        payload: Vec<u8>,
    ) -> Result<()>;
}
