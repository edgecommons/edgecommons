//! `GG_CONFIG` source: Greengrass deployment configuration via IPC
//! (`GetConfiguration` + `SubscribeToConfigurationUpdate`). Phase 2.

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
