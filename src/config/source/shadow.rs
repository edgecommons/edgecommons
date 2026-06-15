//! `SHADOW` source: IoT named device shadow via IPC
//! (`GetThingShadow` + delta subscription, reporting state via `UpdateThingShadow`).
//! Phase 2.

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
