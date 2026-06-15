//! `CONFIG_COMPONENT` source: request/reply to a dedicated config component over
//! messaging. Implemented in Phase 2 (depends on the messaging service).

use async_trait::async_trait;
use serde_json::Value;

use super::ConfigSource;
use crate::error::{GgError, Result};

/// Placeholder for the messaging-backed configuration component source.
#[derive(Default)]
pub struct ConfigComponentSource;

impl ConfigComponentSource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ConfigSource for ConfigComponentSource {
    async fn load(&self) -> Result<Value> {
        Err(GgError::Config(
            "CONFIG_COMPONENT source is not implemented yet (Phase 2)".to_string(),
        ))
    }

    fn source_name(&self) -> &str {
        "CONFIG_COMPONENT"
    }
}
