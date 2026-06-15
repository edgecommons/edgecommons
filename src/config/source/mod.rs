//! # Configuration — sources
//!
//! **One-liner purpose**: Pluggable [`ConfigSource`] implementations and the
//! [`build`] dispatch that selects one from a parsed [`ConfigSourceSpec`].
//!
//! ## Overview
//! Each source loads (and, where applicable, watches) a raw JSON config document.
//! `FILE`/`ENV` are implemented; `GG_CONFIG`/`SHADOW` are gated behind the
//! `greengrass` feature and `CONFIG_COMPONENT` lands in Phase 2.
//!
//! ## Semantics & Architecture
//! - Async trait (`async_trait`) so sources can be held as `Box<dyn ConfigSource>`.
//! - Thread-safety: implementations are `Send + Sync`.
//! - Error handling: [`crate::error::Result`]; selecting an unavailable source
//!   (feature disabled) returns an error rather than silently degrading.
//!
//! ## Usage Example
//! ```no_run
//! use ggcommons::cli::ConfigSourceSpec;
//! use ggcommons::config::source::build;
//! use std::path::PathBuf;
//!
//! # async fn demo() -> ggcommons::Result<()> {
//! let source = build(&ConfigSourceSpec::File { path: PathBuf::from("config.json") })?;
//! let _doc = source.load().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`crate::cli`] (defines [`ConfigSourceSpec`]), [`super::model`].

use async_trait::async_trait;
use serde_json::Value;

use crate::cli::ConfigSourceSpec;
use crate::error::Result;

pub mod config_component;
pub mod env;
pub mod file;
#[cfg(feature = "greengrass")]
pub mod greengrass;
#[cfg(feature = "greengrass")]
pub mod shadow;

/// A source of configuration documents.
#[async_trait]
pub trait ConfigSource: Send + Sync {
    /// Load the current configuration document.
    async fn load(&self) -> Result<Value>;

    /// Short name of the source (for diagnostics).
    fn source_name(&self) -> &str;
}

/// Construct the configuration source for a parsed spec.
pub fn build(spec: &ConfigSourceSpec) -> Result<Box<dyn ConfigSource>> {
    Ok(match spec {
        ConfigSourceSpec::File { path } => Box::new(file::FileConfigSource::new(path.clone())),
        ConfigSourceSpec::Env { var } => Box::new(env::EnvConfigSource::new(var.clone())),
        ConfigSourceSpec::ConfigComponent => {
            Box::new(config_component::ConfigComponentSource::new())
        }
        #[cfg(feature = "greengrass")]
        ConfigSourceSpec::Greengrass { component, key } => {
            Box::new(greengrass::GreengrassConfigSource::new(component.clone(), key.clone()))
        }
        #[cfg(feature = "greengrass")]
        ConfigSourceSpec::Shadow { name } => Box::new(shadow::ShadowConfigSource::new(name.clone())),
        #[cfg(not(feature = "greengrass"))]
        ConfigSourceSpec::Greengrass { .. } | ConfigSourceSpec::Shadow { .. } => {
            return Err(crate::error::GgError::Config(
                "GG_CONFIG/SHADOW sources require the 'greengrass' cargo feature".to_string(),
            ));
        }
    })
}
