//! # Configuration source — FILE
//!
//! **One-liner purpose**: Load configuration from a JSON file on disk.
//!
//! ## Overview
//! Reads and parses the configured path on [`ConfigSource::load`]. File hot-reload
//! via `notify` lands in a later sub-step.
//!
//! ## Semantics & Architecture
//! - Async file read (`tokio::fs`); `Send + Sync`.
//! - Error handling: [`crate::error::GgError::Io`] / [`crate::error::GgError::Json`].
//!
//! ## Usage Example
//! ```no_run
//! use ggcommons::config::source::{file::FileConfigSource, ConfigSource};
//! use std::path::PathBuf;
//! # async fn demo() -> ggcommons::Result<()> {
//! let _doc = FileConfigSource::new(PathBuf::from("config.json")).load().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Related Modules
//! - [`super`].

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
