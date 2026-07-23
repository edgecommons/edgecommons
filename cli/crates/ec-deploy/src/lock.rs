//! The lock file (DESIGN-cli §8.7) — how "no network" becomes literally true.
//!
//! A definition **pins a component version**; the lock **records what that version resolved to**:
//! its immutable digest, the config schema that version publishes (§8.5.5), and its Greengrass
//! component name. This is the `Cargo.lock` pattern. `deployment lock` is the one verb that
//! reaches the network; once it has written the lock, `validate`, `render`, and `plan` are pure
//! functions over files already in Git, and an air-gapped site needs a definition, a lock, and a
//! Git bundle — nothing else.
//!
//! # Degradation is explicit
//!
//! Until components actually publish (RM-013), a pinned version has no resolvable digest. The lock
//! records that as an **unresolved** entry carrying the *reason*, and `validate` warns rather than
//! failing. When the release index appears, the identical code path begins enforcing — no redesign
//! and no flag. What is never acceptable is a lock that looks resolved when nothing was verified.
//!
//! This module performs no I/O: it computes *what* must be locked and assembles the result. The
//! resolution itself goes through [`crate::ports::TargetsPort`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Pin;
use crate::workspace::Workspace;

/// The lock format version. Bumped only for a breaking change to the file's shape.
pub const LOCK_VERSION: u32 = 1;

/// One component version's resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedComponent {
    pub component: String,
    pub version: String,
    /// The immutable artifact digest. Absent while the component publishes no releases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// The Greengrass component name, from the registry's `greengrassComponentName`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greengrass_name: Option<String>,
    /// The config schema *this version* publishes, committed so `validate` stays offline (§8.5.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
    /// Why this entry is not fully resolved. Present exactly when something could not be verified,
    /// so the lock never implies coverage it does not have.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved: Option<String>,
}

impl LockedComponent {
    /// Whether the artifact digest was verified. An unverifiable pin is a warning until RM-013
    /// lands, never a silent pass.
    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.digest.is_some()
    }
}

/// The lock file: every pinned component version in the definition, and what it resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockFile {
    pub lock_version: u32,
    pub definition: String,
    /// Sorted by component then version, so the file is stable across runs and diffs cleanly.
    pub components: Vec<LockedComponent>,
}

impl LockFile {
    #[must_use]
    pub fn lookup(&self, component: &str) -> Option<&LockedComponent> {
        self.components.iter().find(|c| c.component == component)
    }

    /// Components whose digest could not be verified, in file order.
    #[must_use]
    pub fn unverified(&self) -> Vec<&LockedComponent> {
        self.components
            .iter()
            .filter(|c| !c.is_verified())
            .collect()
    }

    /// The lock's own file name for a definition path stem — `site.yaml` locks to `site.lock`.
    #[must_use]
    pub fn file_name_for(definition_stem: &str) -> String {
        format!("{definition_stem}.lock")
    }
}

/// Every distinct component version the definition pins, deduplicated across nodes and sorted.
///
/// A component deployed to twenty nodes at one version is **one** pin: it is the same artifact,
/// and resolving it twenty times would be twenty identical network calls and twenty chances to
/// disagree with itself.
#[must_use]
pub fn pins_for(ws: &Workspace) -> Vec<Pin> {
    let mut seen: BTreeMap<(String, String), Pin> = BTreeMap::new();
    for node in &ws.definition.nodes {
        for comp in &node.components {
            let Some(artifact) = &comp.artifact else {
                continue;
            };
            // Only a pinned version is lockable. A source-form artifact is a development shape
            // with nothing to resolve against.
            let Some(version) = &artifact.version else {
                continue;
            };
            seen.entry((comp.name.clone(), version.clone()))
                .or_insert_with(|| Pin {
                    component: comp.name.clone(),
                    version: version.clone(),
                    greengrass_name: artifact.greengrass_name.clone(),
                    digest: artifact.digest.clone(),
                    config_schema: None,
                });
        }
    }
    seen.into_values().collect()
}

/// Assemble a lock from resolved pins. `resolutions` pairs each requested pin with what the
/// targets port returned, or the reason it could not be resolved.
#[must_use]
pub fn build_lock(definition: &str, resolutions: Vec<(Pin, Result<Pin, String>)>) -> LockFile {
    let mut components: Vec<LockedComponent> = resolutions
        .into_iter()
        .map(|(requested, outcome)| match outcome {
            Ok(resolved) => {
                // A resolution that produced no digest is still unresolved, however it returned.
                let unresolved = resolved.digest.is_none().then(|| {
                    let mut reason = format!(
                        "no release index published for `{}` — its digest cannot be verified \
                         (component release engineering is roadmap RM-013)",
                        requested.component
                    );
                    // A digest typed into the definition is a claim, not evidence. Say plainly
                    // that it was not adopted, so its absence here does not read as a bug.
                    if requested.digest.is_some() {
                        reason.push_str(
                            "; the digest declared in the definition is not recorded as \
                             verified, because nothing verified it",
                        );
                    }
                    reason
                });
                LockedComponent {
                    component: resolved.component,
                    version: resolved.version,
                    digest: resolved.digest,
                    greengrass_name: resolved.greengrass_name.or(requested.greengrass_name),
                    config_schema: resolved.config_schema,
                    unresolved,
                }
            }
            Err(reason) => LockedComponent {
                component: requested.component,
                version: requested.version,
                digest: None,
                greengrass_name: requested.greengrass_name,
                config_schema: None,
                unresolved: Some(reason),
            },
        })
        .collect();
    components.sort_by(|a, b| {
        a.component
            .cmp(&b.component)
            .then_with(|| a.version.cmp(&b.version))
    });
    LockFile {
        lock_version: LOCK_VERSION,
        definition: definition.to_string(),
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(component: &str, version: &str) -> Pin {
        Pin {
            component: component.into(),
            version: version.into(),
            greengrass_name: None,
            digest: None,
            config_schema: None,
        }
    }

    #[test]
    fn a_resolution_without_a_digest_stays_unresolved_and_says_why() {
        let requested = pin("telemetry-processor", "1.4.2");
        let mut resolved = requested.clone();
        resolved.greengrass_name = Some("com.mbreissi.edgecommons.TelemetryProcessor".into());
        let lock = build_lock("site.yaml", vec![(requested, Ok(resolved))]);
        let entry = lock.lookup("telemetry-processor").unwrap();
        assert!(!entry.is_verified());
        assert!(entry.unresolved.as_ref().unwrap().contains("RM-013"));
        // ...but what *was* resolvable is still recorded.
        assert_eq!(
            entry.greengrass_name.as_deref(),
            Some("com.mbreissi.edgecommons.TelemetryProcessor")
        );
        assert_eq!(lock.unverified().len(), 1);
    }

    #[test]
    fn a_resolved_digest_verifies_and_a_failure_records_its_reason() {
        let a = pin("a", "1.0.0");
        let mut a_resolved = a.clone();
        a_resolved.digest = Some("sha256:abc".into());
        let b = pin("b", "2.0.0");
        let lock = build_lock(
            "site.yaml",
            vec![(a, Ok(a_resolved)), (b, Err("registry unreachable".into()))],
        );
        assert!(lock.lookup("a").unwrap().is_verified());
        assert!(!lock.lookup("b").unwrap().is_verified());
        assert_eq!(
            lock.lookup("b").unwrap().unresolved.as_deref(),
            Some("registry unreachable")
        );
        // Sorted by component, so the file diffs cleanly.
        assert_eq!(lock.components[0].component, "a");
        assert_eq!(lock.lock_version, LOCK_VERSION);
    }

    #[test]
    fn the_lock_file_is_named_after_its_definition() {
        assert_eq!(LockFile::file_name_for("site"), "site.lock");
    }
}
