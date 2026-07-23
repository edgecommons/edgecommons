//! The deployment kernel (DESIGN-cli §8).
//!
//! This crate is the hexagonal core: the model, the normalized plan, and the five
//! ports. It performs **no I/O** — every side effect goes through a port, and the
//! adapters live in `ec-adapters`. That is what lets `validate`/`render`/`plan`/`diff`
//! run with no server and no network (RM-012), and what keeps the rule that **no cloud
//! SDK may be linked above the port boundary**: this crate depends on neither.
//!
//! # Status
//!
//! The model, workspace, semantic validator, **HOST renderer**, and release builder are
//! implemented and golden-proven against the Dallas fixture (`deployment-studio` repo),
//! whose render byte-matches the adopted `bottling-company-test` site. The Kubernetes and
//! Greengrass renderers, `deployment lock`, and `deployment diff` are not built yet;
//! `ec-cli` reports [`ec_diag::ExitCode::NotImplemented`] for those rather than pretending
//! a verb exists.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod merge;
pub mod model;
pub mod ports;
pub mod release;
pub mod render;
pub mod validate;
pub mod workspace;

pub use ports::{BlobPort, GitPort, IdentityPort, RunnerPort, TargetsPort};

/// The normative DeploymentDefinition JSON Schema (`edgecommons.io/v1alpha1`), embedded so
/// `deployment validate` stage one runs offline by construction.
pub const DEFINITION_SCHEMA: &str = include_str!("../schema/deployment-definition.schema.json");

/// A deployment target platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Platform {
    Greengrass,
    Host,
    Kubernetes,
}

impl Platform {
    /// Parse a definition's `targetStandard.family` string.
    #[must_use]
    pub fn from_family(family: &str) -> Option<Self> {
        match family {
            "GREENGRASS" => Some(Self::Greengrass),
            "HOST" => Some(Self::Host),
            "KUBERNETES" => Some(Self::Kubernetes),
            _ => None,
        }
    }
}

/// Where a component's configuration comes from at runtime.
///
/// This is the same vocabulary as the runtime `-c/--config <SOURCE>` contract, and it
/// is what selects the **config-stream delivery adapter** (DESIGN-cli §8.5.3): the config
/// is computed once, but how it reaches the device differs per source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfigSource {
    File,
    Env,
    GgConfig,
    Shadow,
    ConfigComponent,
    // The runtime contract spells this CONFIGMAP (one word); SCREAMING_SNAKE_CASE would
    // produce CONFIG_MAP, which no runtime accepts.
    #[serde(rename = "CONFIGMAP")]
    ConfigMap,
}

impl ConfigSource {
    /// Whether a change delivered through this source is picked up **without a restart**.
    ///
    /// This is a property of the *config source*, not of the platform (DESIGN-cli §8.5.4,
    /// D-CLI-14) — which is why restart impact is computed per component and surfaces in
    /// the plan's `restart` consequence group rather than being assumed.
    #[must_use]
    pub fn hot_reloads(self) -> bool {
        match self {
            // Watched / pushed: the library picks the change up live.
            Self::File | Self::ConfigMap | Self::ConfigComponent | Self::Shadow => true,
            // An env change requires a new process; a Greengrass configurationUpdate does
            // not reliably restart the component either, so neither may be assumed live.
            Self::Env | Self::GgConfig => false,
        }
    }

    /// The exact `-c/--config <SOURCE>` string the runtime contract uses.
    #[must_use]
    pub fn as_contract_str(self) -> &'static str {
        match self {
            Self::File => "FILE",
            Self::Env => "ENV",
            Self::GgConfig => "GG_CONFIG",
            Self::Shadow => "SHADOW",
            Self::ConfigComponent => "CONFIG_COMPONENT",
            Self::ConfigMap => "CONFIGMAP",
        }
    }

    /// Whether this source is legal on this platform (`EC2009`).
    #[must_use]
    pub fn is_legal_on(self, platform: Platform) -> bool {
        match self {
            Self::ConfigMap => platform == Platform::Kubernetes,
            Self::GgConfig => platform == Platform::Greengrass,
            _ => true,
        }
    }
}

/// The two release streams (DESIGN-cli §8.5, REVIEW #2).
///
/// They are **independently versioned and independently reconciled**. The `ReleaseLock`
/// correlates them for evidence; it does not fuse them into an atomic apply unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Artifact,
    Config,
}

/// How a change shows up to an operator. The `diff` groups by this, so an operator sees
/// consequences rather than a file-level delta (DESIGN-cli §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Consequence {
    Restart,
    Storage,
    Network,
    Identity,
    Permission,
    Config,
    Artifact,
    ApplyOrder,
}

/// One entry in the normalized plan — the common currency between the CLI, CI, policy,
/// and the UI (DESIGN-cli §8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanEntry {
    /// The node (edge device) this applies to. Greengrass deploys **per thing**, so a
    /// node maps 1:1 onto a deployment (REVIEW #3).
    pub node: String,
    pub component: String,
    pub consequence: Consequence,
    pub summary: String,
    /// Whether applying this entry restarts the component. Derived from the component's
    /// [`ConfigSource`], never assumed.
    pub restarts_component: bool,
}

/// The normalized plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub entries: Vec<PlanEntry>,
}

impl Plan {
    #[must_use]
    pub fn restarts(&self) -> Vec<&PlanEntry> {
        self.entries
            .iter()
            .filter(|e| e.restarts_component)
            .collect()
    }
}

/// A component pin in a definition: a version, and (once locked) an immutable digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pin {
    pub component: String,
    pub version: String,
    /// Resolved by `deployment lock`; absent until then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// The config schema published by *this* version, committed with the lock so that
    /// `validate` can check config against the exact deployed binary offline
    /// (DESIGN-cli §8.5.5, D-CLI-16).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
}

impl Pin {
    /// A pin whose digest could not be resolved is a **warning, not an error**, until
    /// components actually publish releases (RM-013). The tooling says so out loud
    /// rather than implying coverage it does not have.
    #[must_use]
    pub fn is_verifiable(&self) -> bool {
        self.digest.is_some()
    }
}

/// The location of a deployment definition (a folder, or a single file).
#[derive(Debug, Clone)]
pub struct Definition {
    pub path: PathBuf,
}

impl Definition {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_reload_is_a_property_of_the_source_not_the_platform() {
        // Watched or pushed sources pick a change up live...
        assert!(ConfigSource::File.hot_reloads());
        assert!(ConfigSource::ConfigMap.hot_reloads());
        assert!(ConfigSource::ConfigComponent.hot_reloads());
        assert!(ConfigSource::Shadow.hot_reloads());
        // ...while an env change needs a new process, and a Greengrass configurationUpdate
        // does not reliably restart the component either (D-CLI-14).
        assert!(!ConfigSource::Env.hot_reloads());
        assert!(!ConfigSource::GgConfig.hot_reloads());
    }

    #[test]
    fn config_sources_are_platform_gated() {
        assert!(ConfigSource::ConfigMap.is_legal_on(Platform::Kubernetes));
        assert!(!ConfigSource::ConfigMap.is_legal_on(Platform::Host));
        assert!(ConfigSource::GgConfig.is_legal_on(Platform::Greengrass));
        assert!(!ConfigSource::GgConfig.is_legal_on(Platform::Kubernetes));
        // FILE is legal everywhere.
        assert!(ConfigSource::File.is_legal_on(Platform::Host));
        assert!(ConfigSource::File.is_legal_on(Platform::Greengrass));
    }

    #[test]
    fn an_unlocked_pin_is_not_verifiable() {
        let p = Pin {
            component: "telemetry-processor".into(),
            version: "0.3.0".into(),
            digest: None,
            config_schema: None,
        };
        assert!(!p.is_verifiable());
    }

    #[test]
    fn plan_surfaces_restarts() {
        let plan = Plan {
            entries: vec![
                PlanEntry {
                    node: "gw-fill-01".into(),
                    component: "telemetry-processor".into(),
                    consequence: Consequence::Config,
                    summary: "config change picked up live".into(),
                    restarts_component: false,
                },
                PlanEntry {
                    node: "gw-fill-01".into(),
                    component: "modbus-adapter".into(),
                    consequence: Consequence::Restart,
                    summary: "env change forces a restart".into(),
                    restarts_component: true,
                },
            ],
        };
        assert_eq!(plan.restarts().len(), 1);
        assert_eq!(plan.restarts()[0].component, "modbus-adapter");
    }
}
