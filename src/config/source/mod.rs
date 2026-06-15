//! Configuration sources. Each source knows how to load (and, where applicable,
//! watch) a raw JSON config document. The dispatch [`build`] maps a parsed
//! [`ConfigSourceSpec`] to a concrete source.

use async_trait::async_trait;
use serde_json::Value;

use crate::cli::ConfigSourceSpec;
use crate::error::{GgError, Result};

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
            return Err(GgError::Config(
                "GG_CONFIG/SHADOW sources require the 'greengrass' cargo feature".to_string(),
            ));
        }
    })
}
