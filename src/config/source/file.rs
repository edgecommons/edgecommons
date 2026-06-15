//! `FILE` config source. Hot-reload via `notify` lands in Phase 1.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use super::ConfigSource;
use crate::error::Result;

/// Loads configuration from a JSON file on disk.
pub struct FileConfigSource {
    path: PathBuf,
}

impl FileConfigSource {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl ConfigSource for FileConfigSource {
    async fn load(&self) -> Result<Value> {
        let bytes = tokio::fs::read(&self.path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn source_name(&self) -> &str {
        "FILE"
    }
}
