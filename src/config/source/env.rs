//! `ENV` config source: reads a JSON document from an environment variable.

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
