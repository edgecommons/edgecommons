//! # Library crate — the seam, exposed for integration tests
//!
//! The engine is exposed as a library so the thin `main` binary and the integration tests
//! (`tests/`) build against **one** public surface. The binary is a shim: it constructs the
//! `edgecommons` runtime and hands control to [`app::App`]; every unit of real behavior — the
//! device seam ([`device`]), the `sb/*` command surface ([`commands`]), and the operational
//! metrics ([`metrics`]) — lives here, is unit-tested inline, and is driven end-to-end from
//! `tests/live_sim.rs` once you replace the shipped simulator with a real protocol.

pub mod app;
pub mod commands;
pub mod device;
pub mod metrics;
