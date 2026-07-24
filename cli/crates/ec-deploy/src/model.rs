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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hierarchy {
    pub levels: Vec<String>,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetStandard {
    pub family: String,
    #[serde(default)]
    pub exceptions: Vec<TargetException>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetException {
    pub scope: String,
    pub family: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    pub name: String,
    #[serde(default)]
    pub protection: Option<String>,
    pub bindings: String,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    #[serde(default)]
    pub thing_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Auxiliary {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub launch: Option<Launch>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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
    /// The container image, on the Kubernetes profile — the one delivery fact a k8s Deployment
    /// cannot be built without. Absent on HOST/Greengrass (which use `artifact` instead).
    #[serde(default)]
    pub image: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileStage {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    #[serde(default)]
    pub version: Option<String>,
    /// The component's Greengrass component name (e.g. `com.mbreissi.edgecommons.OpcUaAdapter`).
    /// Not derivable from the token, so it is authored; the registry's
    /// `greengrassComponentName` is the canonical source that `deployment lock` will resolve.
    #[serde(default)]
    pub greengrass_name: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub source: Option<ArtifactSource>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Default, Deserialize)]
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

// ---------------------------------------------------------------------------------------------
// Authored form: one shared plant `topology`, deployed to any platform via per-platform
// `profiles`. The types above (`DefinitionDoc` and friends) are the *effective* form the
// renderers consume; [`AuthoredDefinition::effective`] merges a topology with one profile to
// produce it. See `docs/platform/DESIGN-deployment-profiles.md`.
// ---------------------------------------------------------------------------------------------

/// The authored deployment definition.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthoredDefinition {
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub hierarchy: Hierarchy,
    pub topology: Topology,
    /// Per-platform delivery, keyed by a profile name (e.g. `host`, `greengrass`, `kubernetes`).
    pub profiles: IndexMap<String, Profile>,
}

/// The plant, platform-agnostic: what runs where and each component's functional config.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Topology {
    pub nodes: Vec<TopologyNode>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopologyNode {
    pub key: String,
    pub scope: String,
    #[serde(default)]
    pub identity: Option<Identity>,
    #[serde(default)]
    pub labels: Option<IndexMap<String, String>>,
    pub components: Vec<TopologyComponent>,
}

/// A component's functional half: its identity and the config it merges. No delivery details
/// (config source, artifact, launch, image) — those come from a profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TopologyComponent {
    pub name: String,
    #[serde(default)]
    pub catalog_key: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub files: Vec<FileStage>,
    #[serde(default)]
    pub messaging: Option<Messaging>,
}

/// How the topology is delivered on one platform.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub family: String,
    #[serde(default)]
    pub exceptions: Vec<TargetException>,
    pub environments: Vec<Environment>,
    /// Optional selection over the topology. Absent (the norm) deploys the complete plant.
    #[serde(default)]
    pub deploys: Option<Deploys>,
    #[serde(default)]
    pub defaults: Option<ProfileDefaults>,
    /// Per-node platform adornments and the per-component delivery overlay, keyed by node key.
    #[serde(default)]
    pub nodes: IndexMap<String, ProfileNode>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileDefaults {
    /// The config source a component falls back to when it declares none of its own.
    #[serde(default)]
    pub config_source: Option<ConfigSource>,
}

/// A profile's selection over the topology. `nodes` lists the topology nodes this profile deploys
/// (a node not listed is skipped on this platform). `only` optionally narrows a node to a subset of
/// its components; a node absent from `only` deploys all of its components.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Deploys {
    pub nodes: Vec<String>,
    #[serde(default)]
    pub only: IndexMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileNode {
    #[serde(default)]
    pub local_broker: Option<LocalBroker>,
    #[serde(default)]
    pub auxiliaries: Vec<Auxiliary>,
    #[serde(default)]
    pub config_provider: Option<ConfigProvider>,
    #[serde(default)]
    pub components: IndexMap<String, ProfileComponent>,
}

/// A component's delivery half on one platform.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileComponent {
    #[serde(default)]
    pub config_source: Option<ConfigSource>,
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub launch: Option<Launch>,
    #[serde(default)]
    pub image: Option<String>,
}

/// Why an effective definition could not be assembled from a topology + profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveError {
    /// No profile by that name; carries the names that do exist.
    UnknownProfile {
        requested: String,
        available: Vec<String>,
    },
    /// A deployed component resolved to no config source (none on the component, none in defaults).
    NoConfigSource {
        node: String,
        component: String,
        profile: String,
    },
    /// A `deploys` selection named a node or component the topology does not contain.
    UnknownSelection { profile: String, detail: String },
}

impl std::fmt::Display for EffectiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownProfile {
                requested,
                available,
            } => write!(
                f,
                "no profile '{requested}' in the definition (have: {})",
                available.join(", ")
            ),
            Self::NoConfigSource {
                node,
                component,
                profile,
            } => write!(
                f,
                "component '{component}' on node '{node}' has no configSource in profile \
                 '{profile}' — set it on the component or in the profile's defaults"
            ),
            Self::UnknownSelection { profile, detail } => {
                write!(
                    f,
                    "profile '{profile}' deploys {detail}, not in the topology"
                )
            }
        }
    }
}

impl std::error::Error for EffectiveError {}

impl AuthoredDefinition {
    /// Names of every profile, in authored order — for `--profile` help and error messages.
    #[must_use]
    pub fn profile_names(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    /// Merge the topology with one profile into the flat [`DefinitionDoc`] the renderers consume.
    ///
    /// The effective component is the topology component's functional half (`layer`, `messaging`,
    /// `files`, `catalogKey`) plus the profile's delivery half (`configSource`, `artifact`,
    /// `launch`, `image`). A profile deploys the whole topology unless it declares a `deploys`
    /// selection. This is the one place the two halves meet; everything downstream is unchanged.
    pub fn effective(&self, profile_name: &str) -> Result<DefinitionDoc, EffectiveError> {
        let profile =
            self.profiles
                .get(profile_name)
                .ok_or_else(|| EffectiveError::UnknownProfile {
                    requested: profile_name.to_string(),
                    available: self.profile_names(),
                })?;

        // Validate any `deploys` selection against the topology before using it.
        if let Some(deploys) = &profile.deploys {
            for node_key in &deploys.nodes {
                if !self.topology.nodes.iter().any(|n| &n.key == node_key) {
                    return Err(EffectiveError::UnknownSelection {
                        profile: profile_name.to_string(),
                        detail: format!("node '{node_key}'"),
                    });
                }
            }
            for (node_key, comps) in &deploys.only {
                let tn = self.topology.nodes.iter().find(|n| &n.key == node_key);
                let Some(tn) = tn.filter(|_| deploys.nodes.iter().any(|n| n == node_key)) else {
                    return Err(EffectiveError::UnknownSelection {
                        profile: profile_name.to_string(),
                        detail: format!("`only` for node '{node_key}'"),
                    });
                };
                for c in comps {
                    if !tn.components.iter().any(|tc| &tc.name == c) {
                        return Err(EffectiveError::UnknownSelection {
                            profile: profile_name.to_string(),
                            detail: format!("'{c}' on node '{node_key}'"),
                        });
                    }
                }
            }
        }

        let default_source = profile.defaults.as_ref().and_then(|d| d.config_source);
        let mut nodes = Vec::new();
        for tn in &self.topology.nodes {
            // Is this node deployed on this platform, and if so, which of its components?
            // No `deploys` => the whole plant; otherwise membership from `nodes`, component filter
            // from `only` (a node absent from `only` deploys all of its components).
            let only: Option<&Vec<String>> = match &profile.deploys {
                None => None,
                Some(d) => {
                    if !d.nodes.iter().any(|k| k == &tn.key) {
                        continue; // node not deployed on this platform
                    }
                    d.only.get(&tn.key)
                }
            };
            let pnode = profile.nodes.get(&tn.key);
            let mut components = Vec::new();
            for tc in &tn.components {
                if let Some(list) = only {
                    if !list.iter().any(|n| n == &tc.name) {
                        continue;
                    }
                }
                let pc = pnode.and_then(|p| p.components.get(&tc.name));
                let config_source = pc
                    .and_then(|p| p.config_source)
                    .or(default_source)
                    .ok_or_else(|| EffectiveError::NoConfigSource {
                        node: tn.key.clone(),
                        component: tc.name.clone(),
                        profile: profile_name.to_string(),
                    })?;
                components.push(Component {
                    name: tc.name.clone(),
                    catalog_key: tc.catalog_key.clone(),
                    artifact: pc.and_then(|p| p.artifact.clone()),
                    config_source,
                    layer: tc.layer.clone(),
                    files: tc.files.clone(),
                    messaging: tc.messaging.clone(),
                    launch: pc.and_then(|p| p.launch.clone()),
                    image: pc.and_then(|p| p.image.clone()),
                });
            }
            nodes.push(Node {
                key: tn.key.clone(),
                scope: tn.scope.clone(),
                identity: tn.identity.clone(),
                labels: tn.labels.clone(),
                local_broker: pnode.and_then(|p| p.local_broker.clone()),
                auxiliaries: pnode.map(|p| p.auxiliaries.clone()).unwrap_or_default(),
                config_provider: pnode.and_then(|p| p.config_provider.clone()),
                components,
            });
        }

        Ok(DefinitionDoc {
            api_version: self.api_version.clone(),
            kind: self.kind.clone(),
            metadata: self.metadata.clone(),
            hierarchy: self.hierarchy.clone(),
            target_standard: TargetStandard {
                family: profile.family.clone(),
                exceptions: profile.exceptions.clone(),
            },
            environments: profile.environments.clone(),
            nodes,
        })
    }
}

#[cfg(test)]
mod authored_tests {
    use super::*;

    // A two-profile plant: one node, one component, whose functional config lives once in the
    // topology and whose delivery differs per profile.
    const DEF: &str = r#"
apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata: { name: demo }
hierarchy:
  levels: [site, device]
  scopes:
    - { id: site/lab, parent: null }
topology:
  nodes:
    - key: gw-01
      scope: site/lab
      identity: { thingName: gw-01 }
      components:
        - name: opcua-adapter
          catalogKey: OpcUaAdapter
          layer: layers/opcua.json
          messaging: { clientId: gw-opcua, file: opcua-messaging.json }
        - name: telemetry-processor
          layer: layers/telemetry.json
profiles:
  host:
    family: HOST
    environments: [{ name: local, protection: open, bindings: bindings/local.json }]
    defaults: { configSource: CONFIG_COMPONENT }
    nodes:
      gw-01:
        localBroker: { kind: emqx }
        components:
          opcua-adapter:
            artifact: { source: { kind: sibling, repo: opcua-adapter } }
            launch: { order: 30 }
  greengrass:
    family: GREENGRASS
    environments: [{ name: prod, protection: protected, bindings: bindings/prod.json }]
    defaults: { configSource: GG_CONFIG }
    nodes:
      gw-01:
        components:
          opcua-adapter:
            artifact: { version: "1.0.0", digest: "sha256:abc", greengrassName: com.x.OpcUaAdapter }
  kubernetes:
    family: KUBERNETES
    environments: [{ name: local, bindings: bindings/k8s.json }]
    defaults: { configSource: CONFIGMAP }
    deploys: { nodes: [gw-01], only: { gw-01: [opcua-adapter] } }
    nodes:
      gw-01:
        components:
          opcua-adapter: { image: "ghcr.io/x/opcua-adapter:1.0.0" }
"#;

    fn parse() -> AuthoredDefinition {
        serde_yaml::from_str(DEF).expect("authored definition parses")
    }

    #[test]
    fn the_functional_half_is_shared_and_the_delivery_half_is_per_profile() {
        let def = parse();
        let host = def.effective("host").unwrap();
        let gg = def.effective("greengrass").unwrap();

        // Same plant on both: same node, same component identity + functional config.
        assert_eq!(host.target_standard.family, "HOST");
        assert_eq!(gg.target_standard.family, "GREENGRASS");
        for eff in [&host, &gg] {
            let opcua = &eff.nodes[0].components[0];
            assert_eq!(opcua.name, "opcua-adapter");
            assert_eq!(opcua.catalog_key(), "OpcUaAdapter"); // functional, from topology
            assert_eq!(opcua.layer.as_deref(), Some("layers/opcua.json"));
            assert_eq!(opcua.messaging_file(), "opcua-messaging.json");
        }
        // Delivery differs: HOST sources from a sibling repo + supervisord launch; GG is pinned.
        let h_opcua = &host.nodes[0].components[0];
        assert_eq!(h_opcua.config_source, ConfigSource::ConfigComponent);
        assert_eq!(
            h_opcua
                .artifact
                .as_ref()
                .unwrap()
                .source
                .as_ref()
                .unwrap()
                .repo
                .as_deref(),
            Some("opcua-adapter")
        );
        assert_eq!(h_opcua.launch.as_ref().unwrap().order, Some(30));
        assert!(
            host.nodes[0].local_broker.is_some(),
            "HOST node carries the broker"
        );

        let g_opcua = &gg.nodes[0].components[0];
        assert_eq!(g_opcua.config_source, ConfigSource::GgConfig);
        assert_eq!(
            g_opcua.artifact.as_ref().unwrap().digest.as_deref(),
            Some("sha256:abc")
        );
        assert!(g_opcua.launch.is_none(), "GG has no supervisord launch");
        assert!(
            gg.nodes[0].local_broker.is_none(),
            "GG node has no HOST broker"
        );
    }

    #[test]
    fn a_component_with_no_per_component_source_falls_back_to_the_profile_default() {
        // telemetry-processor sets no configSource on any profile; it takes the profile default.
        let def = parse();
        let host = def.effective("host").unwrap();
        let telemetry = host.nodes[0]
            .components
            .iter()
            .find(|c| c.name == "telemetry-processor")
            .unwrap();
        assert_eq!(telemetry.config_source, ConfigSource::ConfigComponent);
    }

    #[test]
    fn deploys_selects_a_subset_and_carries_the_image() {
        // The kubernetes profile deploys only opcua-adapter (not telemetry), and gives it an image.
        let def = parse();
        let k8s = def.effective("kubernetes").unwrap();
        assert_eq!(k8s.nodes[0].components.len(), 1);
        let opcua = &k8s.nodes[0].components[0];
        assert_eq!(opcua.name, "opcua-adapter");
        assert_eq!(
            opcua.image.as_deref(),
            Some("ghcr.io/x/opcua-adapter:1.0.0")
        );
        assert_eq!(opcua.config_source, ConfigSource::ConfigMap);
    }

    #[test]
    fn an_unknown_profile_names_the_ones_that_exist() {
        let def = parse();
        let err = def.effective("openshift").unwrap_err();
        match err {
            EffectiveError::UnknownProfile { available, .. } => {
                assert!(available.contains(&"host".to_string()));
                assert!(available.contains(&"kubernetes".to_string()));
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn a_deploys_selection_naming_an_absent_component_is_rejected() {
        let yaml = DEF.replace(
            "only: { gw-01: [opcua-adapter] }",
            "only: { gw-01: [ghost] }",
        );
        let def: AuthoredDefinition = serde_yaml::from_str(&yaml).unwrap();
        let err = def.effective("kubernetes").unwrap_err();
        assert!(
            matches!(err, EffectiveError::UnknownSelection { .. }),
            "{err}"
        );
    }

    #[test]
    fn a_component_with_no_source_and_no_default_is_an_error() {
        // Strip the kubernetes default so opcua has no configSource anywhere.
        let yaml = DEF.replace("    defaults: { configSource: CONFIGMAP }\n", "");
        let def: AuthoredDefinition = serde_yaml::from_str(&yaml).unwrap();
        let err = def.effective("kubernetes").unwrap_err();
        assert!(
            matches!(err, EffectiveError::NoConfigSource { .. }),
            "{err}"
        );
    }
}
