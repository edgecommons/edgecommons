//! Typed model of the DeploymentDefinition (`edgecommons.io/v1alpha1`).
//!
//! Mirrors the normative JSON Schema ([`crate::DEFINITION_SCHEMA`]); field-level semantics
//! and the semantic rules S-1..S-9 live in the Deployment Studio repo's `schema/DEFINITION.md`
//! and are enforced by [`crate::validate`]. Proven against the Dallas golden fixture
//! (`deployment-studio/fixtures/dallas`), whose render byte-matches the adopted
//! `bottling-company-test` site.

use indexmap::IndexMap;
use serde::Deserialize;

use crate::ConfigSource;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionDoc {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub hierarchy: Hierarchy,
    pub target_standard: TargetStandard,
    pub environments: Vec<Environment>,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hierarchy {
    pub levels: Vec<String>,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub id: String,
    pub parent: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
}

impl Scope {
    /// The `<level>` half of `<level>/<value>`.
    #[must_use]
    pub fn level(&self) -> &str {
        self.id.split('/').next().unwrap_or("")
    }
    /// The `<value>` half of `<level>/<value>`.
    #[must_use]
    pub fn value(&self) -> &str {
        self.id.split_once('/').map(|(_, v)| v).unwrap_or("")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetStandard {
    pub family: String,
    #[serde(default)]
    pub exceptions: Vec<TargetException>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetException {
    pub scope: String,
    pub family: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    pub name: String,
    #[serde(default)]
    pub protection: Option<String>,
    pub bindings: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub key: String,
    pub scope: String,
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub labels: Option<IndexMap<String, String>>,
    #[serde(default)]
    pub local_broker: Option<LocalBroker>,
    #[serde(default)]
    pub auxiliaries: Vec<Auxiliary>,
    #[serde(default)]
    pub config_provider: Option<ConfigProvider>,
    pub components: Vec<Component>,
}

impl Node {
    /// The platform identity bound to the node key (defaults to the key itself — the
    /// strong convention, since the runtime device identity and every UNS topic resolve
    /// from it).
    #[must_use]
    pub fn thing_name(&self) -> &str {
        self.identity
            .as_ref()
            .and_then(|i| i.thing_name.as_deref())
            .unwrap_or(&self.key)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    #[serde(default)]
    pub thing_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalBroker {
    pub kind: String,
    #[serde(default = "default_broker_port")]
    pub port: u16,
    #[serde(default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default)]
    pub launch: Option<Launch>,
}

fn default_broker_port() -> u16 {
    1883
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Auxiliary {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub launch: Option<Launch>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigProvider {
    pub config_source: ConfigSource,
    #[serde(default)]
    pub artifact: Option<Artifact>,
    pub layer: String,
    pub catalog_path: String,
    #[serde(default)]
    pub version_base: Option<String>,
    #[serde(default)]
    pub messaging: Option<Messaging>,
    #[serde(default)]
    pub launch: Option<Launch>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Component {
    pub name: String,
    #[serde(default)]
    pub catalog_key: Option<String>,
    #[serde(default)]
    pub artifact: Option<Artifact>,
    pub config_source: ConfigSource,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub files: Vec<FileStage>,
    #[serde(default)]
    pub messaging: Option<Messaging>,
    #[serde(default)]
    pub launch: Option<Launch>,
}

impl Component {
    /// Catalog display key (default: PascalCase of the component name).
    #[must_use]
    pub fn catalog_key(&self) -> String {
        self.catalog_key.clone().unwrap_or_else(|| {
            self.name
                .split('-')
                .map(|seg| {
                    let mut c = seg.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                })
                .collect()
        })
    }
    /// Messaging file name (default `<name>-messaging.json`).
    #[must_use]
    pub fn messaging_file(&self) -> String {
        self.messaging
            .as_ref()
            .and_then(|m| m.file.clone())
            .unwrap_or_else(|| format!("{}-messaging.json", self.name))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileStage {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub source: Option<ArtifactSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactSource {
    pub kind: String,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Messaging {
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(rename = "type", default)]
    pub type_: Option<String>,
    #[serde(default)]
    pub request_timeout_seconds: Option<u32>,
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Launch {
    #[serde(default)]
    pub order: Option<u32>,
    #[serde(default)]
    pub wait_for: Vec<String>,
    #[serde(default)]
    pub settle_seconds: Option<u32>,
    #[serde(default)]
    pub exec: Option<String>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub env: Option<IndexMap<String, String>>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub start_secs: Option<u32>,
    #[serde(default)]
    pub start_retries: Option<u32>,
    #[serde(default)]
    pub stop_wait_secs: Option<u32>,
}
