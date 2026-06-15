//! Heartbeat subsystem (Phase 1).
//!
//! A `tokio` interval task that samples system metrics via `sysinfo` and emits
//! them through the metric and/or messaging targets. The tick body is wrapped so
//! a transient failure logs and the next tick still fires — the heartbeat can't
//! be permanently killed by one error (unlike the Java `Timer`-based version),
//! and a missing target `config` is handled rather than panicking.
//!
//! Implementation lands in Phase 1.
