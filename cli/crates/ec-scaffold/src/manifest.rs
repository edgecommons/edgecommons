//! Template manifest v2 (DESIGN-cli §5.3).
//!
//! v1 was a bare substitution/rename list, and the CLI carried a hardcoded map of four
//! languages beside it — which is how two complete templates (`java-protocol-adapter`,
//! `python-protocol-adapter`) came to exist in-tree while being unreachable from the CLI
//! (DEF-8). v2 makes the manifest self-describing: it declares its own `language` and
//! `kind`, so **templates are discovered, not registered**, and adding one needs no CLI
//! change. That is the manifest-driven promise the old code made and did not keep.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The archetype axis. Mirrors the registry's own category vocabulary so a scaffolded
/// component and its catalog entry speak the same word (D-CLI-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Service,
    ProtocolAdapter,
    Processor,
    Sink,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::ProtocolAdapter => "protocol-adapter",
            Self::Processor => "processor",
            Self::Sink => "sink",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Language {
    Java,
    Python,
    Rust,
    Typescript,
}

impl Language {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Java => "JAVA",
            Self::Python => "PYTHON",
            Self::Rust => "RUST",
            Self::Typescript => "TYPESCRIPT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Platform {
    Greengrass,
    Host,
    Kubernetes,
}

impl Platform {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Greengrass => "GREENGRASS",
            Self::Host => "HOST",
            Self::Kubernetes => "KUBERNETES",
        }
    }
}

/// A flag-gated group of optional paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conditional {
    /// A condition flag: `dep:local`, `dep:registry`, `kind:processor`, …
    pub when: String,
    pub paths: Vec<String>,
}

/// A file rename, with `{TOKEN}` interpolation in the path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

/// The template manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    /// Must be `2`. A v1 manifest is rejected loudly rather than silently misread.
    pub schema_version: u32,
    /// `<language>/<kind>`, lowercased — must equal the template's directory path.
    pub id: String,
    pub language: Language,
    pub kind: Kind,
    pub description: String,
    /// The platforms this template can emit artifacts for.
    pub platforms: Vec<Platform>,
    /// Tokens that must resolve to a non-empty value.
    #[serde(default)]
    pub requires: Vec<String>,
    /// `relative/path` -> the tokens substituted in it.
    #[serde(default)]
    pub substitutions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub renames: Vec<Rename>,
    /// Platform-gated artifact groups. This is what fixes the asymmetry where Greengrass
    /// artifacts were emitted unconditionally while HOST — a first-class platform — got
    /// nothing at all (DEF-12).
    #[serde(default)]
    pub packs: BTreeMap<Platform, Vec<String>>,
    /// Arbitrary flag-gated paths (`dep:registry`, …).
    #[serde(default)]
    pub conditional: Vec<Conditional>,
}

/// Why a manifest was rejected. A bad manifest is a **hard failure**, not a warning: the
/// Python CLI's plugin loader swallowed a broken command class with `warnings.warn` and the
/// command simply vanished from the surface.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest is not valid JSON: {0}")]
    Json(String),
    #[error("unsupported manifest schemaVersion {found}; this CLI requires 2")]
    Version { found: u32 },
    #[error("manifest id `{id}` does not match its language/kind (`{expected}`)")]
    IdMismatch { id: String, expected: String },
    #[error("manifest declares no platforms")]
    NoPlatforms,
}

impl Manifest {
    /// Parse and self-check a manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError`] if the JSON is malformed, the schema version is not 2, the
    /// declared `id` disagrees with `language`/`kind`, or no platform is declared.
    pub fn parse(json: &str) -> Result<Self, ManifestError> {
        let m: Self = serde_json::from_str(json).map_err(|e| ManifestError::Json(e.to_string()))?;
        if m.schema_version != 2 {
            return Err(ManifestError::Version { found: m.schema_version });
        }
        let expected = m.expected_id();
        if m.id != expected {
            return Err(ManifestError::IdMismatch { id: m.id.clone(), expected });
        }
        if m.platforms.is_empty() {
            return Err(ManifestError::NoPlatforms);
        }
        Ok(m)
    }

    /// The id this manifest's language and kind imply.
    #[must_use]
    pub fn expected_id(&self) -> String {
        format!("{}/{}", self.language.as_str().to_lowercase(), self.kind.as_str())
    }

    #[must_use]
    pub fn supports(&self, platform: Platform) -> bool {
        self.platforms.contains(&platform)
    }

    /// Paths belonging to platforms that were **not** selected, and which must therefore be
    /// pruned from the generated project.
    ///
    /// A path may legitimately belong to **more than one** pack — the `Dockerfile` is used by
    /// both the HOST compose file and the Kubernetes manifests — so a path is pruned only if
    /// **no selected platform also claims it**. Subtracting the kept set is the whole point;
    /// a naive "remove everything the unselected platforms list" would delete the Dockerfile
    /// out from under a HOST-only scaffold.
    #[must_use]
    pub fn pruned_packs(&self, selected: &[Platform]) -> Vec<String> {
        let kept: Vec<&String> = self
            .packs
            .iter()
            .filter(|(p, _)| selected.contains(p))
            .flat_map(|(_, paths)| paths.iter())
            .collect();

        let mut pruned: Vec<String> = self
            .packs
            .iter()
            .filter(|(p, _)| !selected.contains(p))
            .flat_map(|(_, paths)| paths.iter())
            .filter(|path| !kept.contains(path))
            .cloned()
            .collect();
        pruned.sort();
        pruned.dedup();
        pruned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> String {
        serde_json::json!({
            "schemaVersion": 2,
            "id": "rust/service",
            "language": "RUST",
            "kind": "service",
            "description": "Rust component.",
            "platforms": ["GREENGRASS", "HOST", "KUBERNETES"],
            "requires": ["EDGECOMMONS_DEP"],
            "substitutions": { "Cargo.toml": ["BINNAME"] },
            "packs": {
                "GREENGRASS": ["recipe.yaml", "gdk-config.json"],
                "HOST": ["compose.yaml", "supervisor"],
                "KUBERNETES": ["Dockerfile", "k8s"]
            }
        })
        .to_string()
    }

    #[test]
    fn parses_a_v2_manifest() {
        let m = Manifest::parse(&valid()).unwrap();
        assert_eq!(m.language, Language::Rust);
        assert_eq!(m.kind, Kind::Service);
        assert_eq!(m.id, "rust/service");
        assert!(m.supports(Platform::Host));
    }

    #[test]
    fn a_v1_manifest_is_rejected_loudly() {
        // The old manifests had no schemaVersion at all; serde reports the missing field.
        let v1 = r#"{"language":"RUST","description":"x","substitutions":{}}"#;
        let e = Manifest::parse(v1).unwrap_err();
        assert!(matches!(e, ManifestError::Json(_)), "{e:?}");
    }

    #[test]
    fn a_future_schema_version_is_rejected_rather_than_guessed() {
        let m = valid().replace("\"schemaVersion\":2", "\"schemaVersion\":3");
        assert_eq!(Manifest::parse(&m).unwrap_err(), ManifestError::Version { found: 3 });
    }

    #[test]
    fn an_id_that_disagrees_with_language_and_kind_is_caught() {
        let m = valid().replace("\"id\":\"rust/service\"", "\"id\":\"rust/processor\"");
        let e = Manifest::parse(&m).unwrap_err();
        assert_eq!(
            e,
            ManifestError::IdMismatch { id: "rust/processor".into(), expected: "rust/service".into() }
        );
    }

    #[test]
    fn unknown_fields_are_rejected_so_manifest_drift_cannot_ship() {
        let m = valid().replace("\"description\":", "\"nonsense\":\"x\",\"description\":");
        assert!(matches!(Manifest::parse(&m).unwrap_err(), ManifestError::Json(_)));
    }

    #[test]
    fn unselected_platform_packs_are_pruned() {
        let m = Manifest::parse(&valid()).unwrap();
        // A HOST-only scaffold must not carry a Greengrass recipe. Under the Python CLI it
        // did: only Kubernetes was gated, so recipe.yaml and gdk-config.json shipped
        // unconditionally (DEF-12).
        let pruned = m.pruned_packs(&[Platform::Host]);
        assert!(pruned.contains(&"recipe.yaml".to_string()));
        assert!(pruned.contains(&"gdk-config.json".to_string()));
        assert!(pruned.contains(&"k8s".to_string()));
        assert!(!pruned.contains(&"compose.yaml".to_string()));
    }

    #[test]
    fn a_path_shared_by_two_packs_survives_if_either_is_selected() {
        // The Dockerfile is used by BOTH the HOST compose file and the k8s manifests. A naive
        // prune ("delete everything the unselected platforms list") would remove it from a
        // HOST-only scaffold and break the compose build.
        let json = serde_json::json!({
            "schemaVersion": 2,
            "id": "rust/service",
            "language": "RUST",
            "kind": "service",
            "description": "x",
            "platforms": ["HOST", "KUBERNETES"],
            "packs": {
                "HOST": ["compose.yaml", "Dockerfile"],
                "KUBERNETES": ["Dockerfile", "k8s"]
            }
        })
        .to_string();
        let m = Manifest::parse(&json).unwrap();

        let pruned = m.pruned_packs(&[Platform::Host]);
        assert!(pruned.contains(&"k8s".to_string()));
        assert!(!pruned.contains(&"Dockerfile".to_string()), "HOST still needs the Dockerfile");

        let pruned = m.pruned_packs(&[Platform::Kubernetes]);
        assert!(pruned.contains(&"compose.yaml".to_string()));
        assert!(!pruned.contains(&"Dockerfile".to_string()), "KUBERNETES still needs the Dockerfile");
    }

    #[test]
    fn selecting_every_platform_prunes_nothing() {
        let m = Manifest::parse(&valid()).unwrap();
        let all = [Platform::Greengrass, Platform::Host, Platform::Kubernetes];
        assert!(m.pruned_packs(&all).is_empty());
    }

    #[test]
    fn the_protocol_adapter_kind_round_trips() {
        let json = valid()
            .replace("\"id\":\"rust/service\"", "\"id\":\"rust/protocol-adapter\"")
            .replace("\"kind\":\"service\"", "\"kind\":\"protocol-adapter\"");
        let m = Manifest::parse(&json).unwrap();
        assert_eq!(m.kind, Kind::ProtocolAdapter);
        assert_eq!(m.id, "rust/protocol-adapter");
    }
}
