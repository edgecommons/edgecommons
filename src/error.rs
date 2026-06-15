//! Library error type. The library never calls `std::process::exit`; every
//! fallible path returns `Result<T, GgError>` and the application decides how to
//! handle failure (a deliberate departure from the Java library, which exits the
//! host JVM in 18 places).

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

    /// Metrics emission error.
    #[error("metrics error: {0}")]
    Metrics(String),

    /// Greengrass IPC error (Phase 2).
    #[error("Greengrass IPC error: {0}")]
    Ipc(String),

    /// Underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
