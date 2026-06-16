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
//! // CONFIG_COMPONENT needs a messaging service + identity; other sources ignore them.
//! let source = build(
//!     &ConfigSourceSpec::File { path: PathBuf::from("config.json") },
//!     None,
//!     "my-thing",
//!     "com.example.MyComponent",
//! )?;
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

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::cli::ConfigSourceSpec;
use crate::error::Result;
use crate::messaging::MessagingService;

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

    /// Begin watching for changes, returning a receiver of new raw config documents.
    ///
    /// Returns `None` for sources that don't support hot reload. The source must be
    /// kept alive for the receiver to keep producing (it may own an OS watcher).
    fn watch(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<Value>> {
        None
    }
}

/// Construct the configuration source for a parsed spec.
///
/// `messaging` (and the `thing_name` / `component_name` identity) are required only
/// by the `CONFIG_COMPONENT` source; the other sources ignore them.
pub fn build(
    spec: &ConfigSourceSpec,
    messaging: Option<Arc<dyn MessagingService>>,
    thing_name: &str,
    component_name: &str,
) -> Result<Box<dyn ConfigSource>> {
    Ok(match spec {
        ConfigSourceSpec::File { path } => Box::new(file::FileConfigSource::new(path.clone())),
        ConfigSourceSpec::Env { var } => Box::new(env::EnvConfigSource::new(var.clone())),
        ConfigSourceSpec::ConfigComponent => {
            let messaging = messaging.ok_or_else(|| {
                crate::error::GgError::Config(
                    "CONFIG_COMPONENT source requires a messaging service (run in a mode that provides one)".to_string(),
                )
            })?;
            Box::new(config_component::ConfigComponentSource::new(
                messaging,
                thing_name,
                component_name,
            ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ConfigSourceSpec;
    use std::path::PathBuf;

    #[test]
    fn builds_file_and_env_sources() {
        let file = build(
            &ConfigSourceSpec::File { path: PathBuf::from("config.json") },
            None,
            "thing",
            "comp",
        )
        .unwrap();
        assert_eq!(file.source_name(), "FILE");

        let env = build(&ConfigSourceSpec::Env { var: "CONFIG".into() }, None, "thing", "comp").unwrap();
        assert_eq!(env.source_name(), "ENV");
    }

    #[test]
    fn config_component_requires_a_messaging_service() {
        let result = build(&ConfigSourceSpec::ConfigComponent, None, "thing", "comp");
        assert!(result.is_err(), "CONFIG_COMPONENT needs messaging");
    }

    #[test]
    fn config_component_builds_with_messaging() {
        let svc: Arc<dyn MessagingService> = crate::testutil::RecordingMessaging::new();
        let source =
            build(&ConfigSourceSpec::ConfigComponent, Some(svc), "thing", "comp").unwrap();
        assert_eq!(source.source_name(), "CONFIG_COMPONENT");
    }

    #[cfg(not(feature = "greengrass"))]
    #[test]
    fn greengrass_sources_require_the_feature() {
        assert!(build(
            &ConfigSourceSpec::Greengrass { component: None, key: "ComponentConfig".into() },
            None,
            "thing",
            "comp"
        )
        .is_err());
        assert!(build(&ConfigSourceSpec::Shadow { name: None }, None, "thing", "comp").is_err());
    }
}
