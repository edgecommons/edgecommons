//! # Configuration source — GG_CONFIG
//!
//! **One-liner purpose**: Load configuration from the Greengrass deployment via
//! IPC (`GetConfiguration` + `SubscribeToConfigurationUpdate`).
//!
//! ## Overview
//! Placeholder until Phase 2: [`ConfigSource::load`] returns a clear "not
//! implemented" error. Compiled only with the `greengrass` feature.
//!
//! ## Semantics & Architecture
//! - `Send + Sync`; will use the Greengrass component SDK once implemented.
//! - Error handling: currently always [`crate::error::GgError::Ipc`].
//!
//! ## Related Modules
//! - [`super`].

use async_trait::async_trait;
use serde_json::Value;

use super::ConfigSource;
use crate::error::{GgError, Result};

/// Placeholder for the Greengrass-IPC-backed configuration source.
pub struct GreengrassConfigSource {
    #[allow(dead_code)]
    component: Option<String>,
    #[allow(dead_code)]
    key: String,
}

impl GreengrassConfigSource {
    pub fn new(component: Option<String>, key: String) -> Self {
        Self { component, key }
    }
}

#[async_trait]
impl ConfigSource for GreengrassConfigSource {
    async fn load(&self) -> Result<Value> {
        Err(GgError::Ipc(
            "GG_CONFIG source is not implemented yet (Phase 2)".to_string(),
        ))
    }

    fn source_name(&self) -> &str {
        "GG_CONFIG"
    }
}
