//! # Configuration source — SHADOW
//!
//! **One-liner purpose**: Load configuration from an IoT named device shadow via
//! IPC (`GetThingShadow` + delta subscription, reporting via `UpdateThingShadow`).
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

/// Placeholder for the device-shadow-backed configuration source.
pub struct ShadowConfigSource {
    #[allow(dead_code)]
    name: Option<String>,
}

impl ShadowConfigSource {
    pub fn new(name: Option<String>) -> Self {
        Self { name }
    }
}

#[async_trait]
impl ConfigSource for ShadowConfigSource {
    async fn load(&self) -> Result<Value> {
        Err(GgError::Ipc(
            "SHADOW source is not implemented yet (Phase 2)".to_string(),
        ))
    }

    fn source_name(&self) -> &str {
        "SHADOW"
    }
}
