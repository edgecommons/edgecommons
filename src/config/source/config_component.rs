//! # Configuration source — CONFIG_COMPONENT
//!
//! **One-liner purpose**: Load configuration from a dedicated config component via
//! request/reply over messaging.
//!
//! ## Overview
//! Placeholder until Phase 2: [`ConfigSource::load`] returns a clear "not
//! implemented" error rather than silently succeeding.
//!
//! ## Semantics & Architecture
//! - `Send + Sync`; will depend on the messaging service once implemented.
//! - Error handling: currently always [`crate::error::GgError::Config`].
//!
//! ## Related Modules
//! - [`super`], [`crate::messaging`].

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
