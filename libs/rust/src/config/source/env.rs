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

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_var(prefix: &str) -> String {
        format!("GGC_{prefix}_{}", uuid::Uuid::new_v4().simple())
    }

    #[tokio::test]
    async fn loads_json_from_env_var() {
        let var = unique_var("OK");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var(&var, r#"{ "a": 1, "b": "x" }"#) };
        let src = EnvConfigSource::new(var.clone());
        let doc = src.load().await.unwrap();
        assert_eq!(doc["a"], 1);
        assert_eq!(doc["b"], "x");
        assert_eq!(src.source_name(), "ENV");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var(&var) };
    }

    #[tokio::test]
    async fn missing_var_is_config_error() {
        let var = unique_var("MISSING");
        let err = EnvConfigSource::new(var).load().await.unwrap_err();
        assert!(matches!(err, GgError::Config(_)));
    }

    #[tokio::test]
    async fn invalid_json_is_error() {
        let var = unique_var("BAD");
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::set_var(&var, "this is not json") };
        let result = EnvConfigSource::new(var.clone()).load().await;
        assert!(result.is_err());
        // FIXME: Audit that the environment access only happens in single-threaded code.
        unsafe { std::env::remove_var(&var) };
    }
}
