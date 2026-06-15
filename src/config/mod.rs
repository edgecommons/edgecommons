//! Configuration subsystem: typed model, schema validation, template-variable
//! substitution, and pluggable sources.
//!
//! Config is published as an immutable [`model::Config`] snapshot. Hot-reload
//! (Phase 1) swaps the snapshot atomically via `ArcSwap` and notifies subscribers
//! through a `tokio::sync::watch` channel — no in-place mutation of shared state.

pub mod model;
pub mod source;
pub mod template;
pub mod validation;

pub use model::Config;
