//! # Error
//!
//! **One-liner purpose**: The library-wide error type [`GgError`] and [`Result`] alias.
//!
//! ## Overview
//! Every fallible path in the library returns `Result<T, GgError>`. The library
//! **never** calls `std::process::exit` (a deliberate departure from the Java
//! library, which exits the host JVM in 18 places) — the application decides how
//! to handle failure.
//!
//! ## Semantics & Architecture
//! - `GgError` is a `thiserror` enum with one variant per subsystem plus `From`
//!   conversions for `std::io::Error` and `serde_json::Error`.
//! - Thread-safety: the type is `Send + Sync`.
//! - Error handling strategy for the whole crate: typed errors via `thiserror`;
//!   `anyhow` is reserved for binaries/examples.
//!
//! ## Usage Example
//! ```
//! use ggcommons::{GgError, Result};
//!
//! fn fallible() -> Result<()> {
//!     Err(GgError::Config("bad config".into()))
//! }
//! assert!(fallible().is_err());
//! ```
//!
//! ## Design Choices
//! Subsystem-keyed variants (`Config`, `Messaging`, …) keep call-site mapping
//! explicit and let callers match on failure category without string parsing.
//!
//! ## Safety & Panics
//! None; constructing or matching errors cannot panic.
//!
//! ## Related Modules
//! Used by every module in the crate.

use thiserror::Error;

/// Convenience alias for `Result<T, GgError>`.
pub type Result<T> = std::result::Result<T, GgError>;

/// All errors surfaced by the library, grouped by subsystem.
#[derive(Debug, Error)]
pub enum GgError {
    /// Command-line parsing / contract violation (e.g. STANDALONE without a path).
    #[error("CLI error: {0}")]
    Cli(String),

    /// Configuration loading or shape error.
    #[error("configuration error: {0}")]
    Config(String),

    /// JSON-schema validation failure.
    #[error("configuration validation failed: {0}")]
    Validation(String),

    /// Messaging (MQTT / IPC) error.
    #[error("messaging error: {0}")]
    Messaging(String),

    /// UNS topic/token validation failure (UNS-CANONICAL-DESIGN §2.2). Carries the
    /// machine-readable [`crate::uns::UnsValidationCode`] pinned in
    /// `uns-test-vectors/topics.json` so all four languages fail identically.
    #[error("UNS validation failed [{code}]: {detail}")]
    UnsValidation {
        /// The machine-readable failure code (the exact §2.2 set).
        code: crate::uns::UnsValidationCode,
        /// Human-readable detail (never parsed by tests — the code is the contract).
        detail: String,
    },

    /// Command-inbox registration error (DESIGN-uns §9.5, the `commands()` facade): a verb
    /// collides with a built-in, a delegated verb (e.g. `set-config`), or an
    /// already-registered custom verb ([`crate::commands::CommandInbox::register`]).
    #[error("command error: {0}")]
    Command(String),

    /// Reserved-class publish-guard rejection (UNS-CANONICAL-DESIGN §4.1, D-U4/D-U8/D-U24):
    /// a client-chosen topic targeted a library-owned UNS class
    /// (`state | metric | cfg | log`). The message names the topic and class and points
    /// at the library publishers. The guard is misuse prevention, not a security
    /// boundary — per-device broker ACLs are the durable enforcement.
    #[error("reserved UNS topic: {0}")]
    ReservedTopic(String),

    /// The framework-owned `request()` deadline fired before a reply arrived
    /// (UNS-CANONICAL-DESIGN §5, D-U5). The ephemeral reply subscription has already
    /// been cleaned up when this surfaces; a retry must issue a fresh request.
    #[error("request timed out after {secs}s waiting for a reply (request topic '{topic}')")]
    RequestTimeout {
        /// The topic the request was published to.
        topic: String,
        /// The deadline that fired, in seconds.
        secs: f64,
    },

    /// Metrics emission error.
    #[error("metrics error: {0}")]
    Metrics(String),

    /// Greengrass IPC error.
    #[error("Greengrass IPC error: {0}")]
    Ipc(String),

    /// Telemetry-streaming error (the `streaming` feature).
    #[error("streaming error: {0}")]
    Streaming(String),

    /// Credentials / local vault error (the `credentials` feature).
    #[error("credentials error: {0}")]
    Credentials(String),

    /// Parameter service error (the `parameters` feature).
    #[error("parameters error: {0}")]
    Parameters(String),

    /// Underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
