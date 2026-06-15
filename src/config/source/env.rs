//! # Configuration source — ENV
//!
//! **One-liner purpose**: Load configuration from a JSON document held in an
//! environment variable (default `CONFIG`).
//!
//! ## Semantics & Architecture
//! - No hot-reload (environment is fixed for the process lifetime); `Send + Sync`.
//! - Error handling: [`crate::error::GgError::Config`] if the variable is unset,
//!   [`crate::error::GgError::Json`] if it is not valid JSON.
//!
//! ## Usage Example
//! ```no_run
//! use ggcommons::config::source::{env::EnvConfigSource, ConfigSource};
//! # async fn demo() -> ggcommons::Result<()> {
//! let _doc = EnvConfigSource::new("CONFIG".to_string()).load().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Related Modules
//! - [`super`].

use async_trait::async_trait;
use serde_json::Value;

use super::ConfigSource;
use crate::error::{GgError, Result};

/// Loads configuration from an environment variable (default `CONFIG`).
pub struct EnvConfigSource {
    var: String,
}

impl EnvConfigSource {
    pub fn new(var: String) -> Self {
        Self { var }
    }
}

#[async_trait]
impl ConfigSource for EnvConfigSource {
    async fn load(&self) -> Result<Value> {
        let raw = std::env::var(&self.var)
            .map_err(|_| GgError::Config(format!("environment variable '{}' is not set", self.var)))?;
        Ok(serde_json::from_str(&raw)?)
    }

    fn source_name(&self) -> &str {
        "ENV"
    }
}
