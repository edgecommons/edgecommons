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

pub mod candidate;
pub(crate) mod effective;
pub(crate) mod identity;
pub(crate) mod layered;
pub mod model;
pub mod source;
pub mod template;
pub mod validation;

pub use candidate::{
    ConfigurationCandidateValidator, ConfigurationValidationError, ConfigurationValidationPhase,
    ConfigurationValidationResult, DEFAULT_CANDIDATE_VALIDATION_TIMEOUT,
    MAX_CANDIDATE_VALIDATION_TIMEOUT,
};
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

/// A listener that prepares an atomic configuration application transaction.
///
/// Register exactly one coordinator with
/// [`crate::EdgeCommons::add_config_apply_listener`] when a dependent runtime must either accept
/// the entire candidate or keep operating on its existing generation. A coordinator, rather than
/// independently-applied listeners, makes a rejection atomic: a later listener cannot reject a
/// candidate after an earlier listener has already changed its runtime. The coordinator must
/// prepare without changing its live runtime; Core calls the returned transaction's `commit` while
/// the old Core snapshot remains active, then publishes the new snapshot only after commit
/// succeeds. On a failed commit, Core calls `rollback` before rejecting the candidate. `commit`
/// is deliberately not externally cancelled: it performs a potentially destructive runtime
/// transition and must use its own bounded stages and return only after success or restoration.
#[async_trait]
pub trait ConfigurationApplyListener: Send + Sync {
    /// Prepares an application transaction for `config` without changing the live runtime.
    ///
    /// The returned transaction owns all staged resources. If preparation fails, it must leave
    /// the existing runtime intact; if it succeeds, its [`PreparedConfigurationApply::commit`]
    /// and [`PreparedConfigurationApply::rollback`] methods are called while the configuration
    /// lifecycle remains serialized.
    async fn prepare_configuration_apply(
        &self,
        config: Arc<Config>,
    ) -> ConfigurationApplicationResult<Box<dyn PreparedConfigurationApply>>;
}

/// A prepared, rollback-capable configuration application transaction.
///
/// Core invokes `commit` before it stores the candidate as the active configuration. If `commit`
/// fails, Core invokes `rollback` and keeps the prior snapshot and generation. `commit` and
/// `rollback` must apply their own bounded deadlines: Core awaits them rather than dropping either
/// future midway through a destructive transition. Implementations must make `rollback` restore
/// the prior runtime before reporting success; cleanup of staged-but-never-committed resources
/// belongs in `Drop`.
#[async_trait]
pub trait PreparedConfigurationApply: Send {
    /// Transitions the prepared runtime to the candidate configuration.
    async fn commit(&mut self) -> ConfigurationApplicationResult<()>;

    /// Restores the runtime that was active before [`Self::commit`] was attempted.
    async fn rollback(&mut self) -> ConfigurationApplicationResult<()>;
}

/// An operator-safe failure from a configuration application transaction.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("configuration application failed [{code}]: {message}")]
pub struct ConfigurationApplicationError {
    /// Stable machine-readable failure code.
    pub code: String,
    /// Sanitized, bounded diagnostic suitable for operators.
    pub message: String,
}

impl ConfigurationApplicationError {
    /// Constructs an operator-safe transaction failure.
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: candidate::sanitize(&message.into()),
        }
    }
}

/// Result returned by [`ConfigurationApplyListener`] and [`PreparedConfigurationApply`].
pub type ConfigurationApplicationResult<T> = std::result::Result<T, ConfigurationApplicationError>;

/// Why installing a pre-commit configuration coordinator failed.
///
/// There can be only one [`ConfigurationApplyListener`]. A runtime that owns several dependent
/// services must coordinate them inside that one listener so it can keep its own generation
/// atomic when a candidate is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigurationApplyListenerRegistrationError {
    /// A coordinator is already installed for this [`crate::EdgeCommons`] instance.
    #[error("a configuration-application coordinator is already registered")]
    AlreadyRegistered,
    /// The listener registry was poisoned by a panic and can no longer be trusted.
    #[error("the configuration-application coordinator registry is unavailable")]
    RegistryUnavailable,
}
