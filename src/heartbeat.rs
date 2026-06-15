//! # Heartbeat
//!
//! **One-liner purpose**: Periodically sample and emit system health metrics.
//!
//! ## Overview
//! A `tokio` interval task that samples system metrics via `sysinfo` and emits
//! them through the metric and/or messaging targets.
//!
//! ## Semantics & Architecture
//! - The tick body is wrapped so a transient failure logs and the next tick still
//!   fires — the heartbeat can't be permanently killed by one error (unlike the
//!   Java `Timer`-based version), and a missing target `config` is handled rather
//!   than panicking.
//! - Error handling: per-tick failures are logged, never propagated as a panic.
//!
//! ## Design Choices
//! `tokio` interval task over `std`/`Timer` so a panic is isolated and the loop is
//! cancellable via task abort (RAII).
//!
//! ## Status
//! Stub — implementation lands in a later Phase 1 sub-step.
//!
//! ## Related Modules
//! - [`crate::metrics`], [`crate::messaging`].
