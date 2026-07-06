//! # Configuration
//!
//! **One-liner purpose**: Typed config model, schema validation, template-variable
//! substitution, and pluggable sources.
//!
//! ## Overview
//! Configuration is loaded by a [`source::ConfigSource`], validated against the
//! embedded JSON schema ([`validation`]), and exposed as an immutable
//! [`model::Config`] snapshot. Template placeholders are resolved by [`template`].
//!
//! ## Semantics & Architecture
//! - The snapshot is published as `Arc<Config>` and (in later sub-steps) swapped
//!   atomically via `ArcSwap`, with change notification over a `tokio::sync::watch`
//!   channel — never in-place mutation of shared state.
//! - Thread-safety: `Config` is immutable and `Send + Sync`; readers always see a
//!   consistent snapshot.
//! - Error handling: [`crate::error::Result`] throughout; validation is fail-closed.
//!
//! ## Usage Example
//! ```
//! use edgecommons::config::model::Config;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("com.example.C", "thing-1", json!({ "tags": { "site": "f1" } })).unwrap();
//! assert_eq!(cfg.thing_name, "thing-1");
//! ```
//!
//! ## Design Choices
//! Typed `serde` structs cover the known sections while the raw JSON is retained
//! for template substitution over arbitrary user keys and instance subtrees.
//!
//! ## Safety & Panics
//! None in normal operation.
//!
//! ## Related Modules
//! - [`model`], [`validation`], [`template`], [`source`].

pub(crate) mod effective;
pub(crate) mod identity;
pub mod model;
pub mod source;
pub mod template;
pub mod validation;

pub use model::Config;

use std::sync::Arc;

use async_trait::async_trait;

/// A listener notified after the configuration is hot-reloaded.
///
/// Mirrors the Java/Python `ConfigurationChangeListener`. Register one with
/// [`crate::EdgeCommons::add_config_change_listener`]. Implementations should be quick
/// or spawn their own work; the return value indicates whether the change was
/// handled (kept for parity with the Java/Python contract).
#[async_trait]
pub trait ConfigurationChangeListener: Send + Sync {
    /// Called with the new configuration snapshot after a successful reload.
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool;
}
