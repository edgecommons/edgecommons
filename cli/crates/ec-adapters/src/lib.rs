//! Adapters behind the five deployment ports (DESIGN-cli §8.2).
//!
//! **This is the only crate permitted to link a cloud SDK, and only in the Greengrass
//! adapter.** Everything above the port boundary — `ec-deploy`, `ec-validate`,
//! `ec-scaffold`, `ec-cli` — must stay free of one. Today nothing here links one either:
//! the Greengrass work shells out to `gdk` and `greengrass-cli`, exactly as the Python CLI
//! did, which already satisfies the rule.
//!
//! # Status
//!
//! The local adapters are Phase P4, alongside the renderers they serve. The types below
//! exist so the port boundary compiles and is reviewable now.

use std::path::{Path, PathBuf};

use ec_deploy::ports::{LocalRoot, PortError};

/// The Git port backed by a plain local clone — no Git host, no network.
///
/// This is what makes the governance story testable at zero cost and makes an air-gapped
/// site ordinary rather than a special mode: a definition, a lock, and a Git bundle on
/// removable media is a complete workflow.
pub struct LocalGit {
    pub root: LocalRoot,
}

/// The Blob port backed by the filesystem.
pub struct FsBlob {
    pub root: PathBuf,
}

/// The Runner port backed by a local subprocess.
///
/// The Runner is where target credentials live — never in the tool itself (D-CLI-8,
/// D-CLI-10).
pub struct SubprocessRunner;

/// An external tool the adapters shell out to. Shelling out is deliberate: it is what
/// keeps `boto3`-shaped dependencies (and their cloud SDKs) out of the binary entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalTool {
    Gdk,
    GreengrassCli,
    Kubectl,
    Docker,
    Helm,
    Aws,
}

impl ExternalTool {
    #[must_use]
    pub fn binary(self) -> &'static str {
        match self {
            Self::Gdk => "gdk",
            Self::GreengrassCli => "greengrass-cli",
            Self::Kubectl => "kubectl",
            Self::Docker => "docker",
            Self::Helm => "helm",
            Self::Aws => "aws",
        }
    }
}

/// Locate an external tool on `PATH`, or report an environment failure naming it.
pub fn require(tool: ExternalTool) -> Result<PathBuf, PortError> {
    which(tool.binary())
        .ok_or_else(|| PortError::Unavailable(format!("{} not found on PATH", tool.binary())))
}

/// Minimal `which`, so the CLI does not take a dependency for a dozen lines.
#[must_use]
pub fn which(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    // On Windows a bare name resolves through PATHEXT; check the common cases so `doctor`
    // does not report a tool missing purely because it is `gdk.cmd` rather than `gdk`.
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_lowercase)
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{binary}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The result of loading a deployment workspace from the filesystem: the parsed model plus
/// every referenced file's text, rooted at the definition's directory.
pub struct LoadedWorkspace {
    pub workspace: ec_deploy::workspace::Workspace,
    pub root: PathBuf,
    pub definition_text: String,
    /// The lock committed beside the definition, when `deployment lock` has been run. It supplies
    /// what the definition itself need not carry — today the Greengrass component name (§8.7).
    pub lock: Option<ec_deploy::lock::LockFile>,
}

/// Load a definition (file path) and every file it references. This is the I/O half the
/// kernel refuses to do: the kernel names the paths, the adapter reads them.
pub fn load_workspace(definition: &Path) -> Result<LoadedWorkspace, String> {
    let definition_text = std::fs::read_to_string(definition)
        .map_err(|e| format!("reading {}: {e}", definition.display()))?;
    let doc =
        ec_deploy::workspace::parse_definition(&definition_text).map_err(|e| e.to_string())?;
    let root = definition
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut files = std::collections::BTreeMap::new();
    for rel in ec_deploy::workspace::referenced_paths(&doc) {
        let path = root.join(&rel);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "reading {} (referenced by the definition): {e}",
                path.display()
            )
        })?;
        files.insert(rel, text);
    }
    let lock = read_lock(definition)?;
    Ok(LoadedWorkspace {
        workspace: ec_deploy::workspace::Workspace {
            definition: doc,
            files,
        },
        root,
        definition_text,
        lock,
    })
}

/// The lock path for a definition — `site.yaml` locks to `site.lock`, beside it.
#[must_use]
pub fn lock_path_for(definition: &Path) -> PathBuf {
    let stem = definition
        .file_stem()
        .map_or_else(|| "deployment".to_string(), |s| s.to_string_lossy().into());
    definition
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(ec_deploy::lock::LockFile::file_name_for(&stem))
}

/// Read the lock beside a definition. Absent is fine — locking is a separate step. A lock that is
/// *present but unreadable* is an error: silently ignoring it would drop the very facts the
/// definition is relying on it for, and surface later as a confusing missing-name failure.
pub fn read_lock(definition: &Path) -> Result<Option<ec_deploy::lock::LockFile>, String> {
    let path = lock_path_for(definition);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let lock: ec_deploy::lock::LockFile = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a readable lock file: {e}", path.display()))?;
    if lock.lock_version != ec_deploy::lock::LOCK_VERSION {
        return Err(format!(
            "{} is lock version {}, but this build understands version {}. \
             Re-run `edgecommons deployment lock`.",
            path.display(),
            lock.lock_version,
            ec_deploy::lock::LOCK_VERSION
        ));
    }
    Ok(Some(lock))
}

/// `<HEAD commit>` or `<HEAD commit>-dirty`, judged against `dir`; `None` outside a repo.
#[must_use]
pub fn describe_head(dir: &Path) -> Option<String> {
    let run = |args: &[&str]| -> Option<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let commit = run(&["rev-parse", "HEAD"])?;
    let dirty = run(&["status", "--porcelain", "--", "."])
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    Some(if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    })
}

// --- Registry-backed pin resolution (the Targets port) --------------------------------------

/// Where the ecosystem catalog is read from, and how.
///
/// Shelling out to `gh` rather than embedding an HTTP client and a token store is deliberate: it
/// keeps credentials out of this binary entirely (D-CLI-8), which is the same reason apply lives
/// behind the Runner port.
pub const REGISTRY_REPO: &str = "edgecommons/registry";
pub const REGISTRY_PATH: &str = "components.json";

/// Load the ecosystem catalog: an explicit local path, or the registry via an authenticated `gh`.
///
/// This is **the** catalog loader — the CLI's `registry` verbs and `deployment lock` share it, so
/// there is one notion of where the catalog comes from rather than two that can disagree.
pub fn load_catalog(source: Option<&str>) -> Result<serde_json::Value, PortError> {
    match source {
        Some(s) if Path::new(s).is_file() => {
            let text =
                std::fs::read_to_string(s).map_err(|e| PortError::Other(format!("{s}: {e}")))?;
            serde_json::from_str(&text).map_err(|e| PortError::Other(format!("{s}: {e}")))
        }
        Some(s) if s.starts_with("http://") || s.starts_with("https://") => {
            Err(PortError::Unavailable(format!(
                "fetching a registry over HTTP is not supported by this build ({s}). \
                 Pass a local path, or rely on the default `gh`-authenticated read."
            )))
        }
        Some(s) => Err(PortError::NotFound(format!("no such registry file: {s}"))),
        None => load_catalog_via_gh(),
    }
}

fn load_catalog_via_gh() -> Result<serde_json::Value, PortError> {
    if which("gh").is_none() {
        return Err(PortError::Unavailable(
            "gh not found on PATH — needed to read the private edgecommons registry. \
             Install it, or pass --source <path|url>."
                .into(),
        ));
    }
    let out = std::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{REGISTRY_REPO}/contents/{REGISTRY_PATH}?ref=main"),
            "-H",
            "Accept: application/vnd.github.raw",
        ])
        .output()
        .map_err(|e| PortError::Unavailable(format!("gh failed: {e}")))?;
    if !out.status.success() {
        return Err(PortError::Unavailable(format!(
            "gh could not read {REGISTRY_REPO}/{REGISTRY_PATH}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| PortError::Other(format!("the registry is not valid JSON: {e}")))
}

/// The Targets port backed by the ecosystem registry.
///
/// This is the **one** networked operation in the binary, and it happens only in
/// `deployment lock` (DESIGN-cli §8.7). It resolves what the registry can answer today — the
/// Greengrass component name — and reports honestly that digests and per-version config schemas
/// await the release index (RM-013) rather than fabricating them.
pub struct RegistryTargets {
    catalog: serde_json::Value,
}

impl RegistryTargets {
    /// Load the catalog once, up front: locking twenty components must not mean twenty fetches.
    pub fn load(source: Option<&str>) -> Result<Self, PortError> {
        Ok(Self {
            catalog: load_catalog(source)?,
        })
    }

    fn entry(&self, component: &str) -> Option<&serde_json::Value> {
        self.catalog
            .get("components")?
            .as_array()?
            .iter()
            .find(|c| c.get("name").and_then(|n| n.as_str()) == Some(component))
    }
}

impl ec_deploy::ports::TargetsPort for RegistryTargets {
    fn resolve_pin(&self, pin: &ec_deploy::Pin) -> Result<ec_deploy::Pin, PortError> {
        let entry = self.entry(&pin.component).ok_or_else(|| {
            PortError::NotFound(format!("no component `{}` in the registry", pin.component))
        })?;
        // Built from what the registry knows, **not** cloned from the request. A digest the author
        // typed into the definition is a claim, not evidence; carrying it through would produce a
        // lock that reports a verified digest when nothing verified anything.
        Ok(ec_deploy::Pin {
            component: pin.component.clone(),
            version: pin.version.clone(),
            // Resolvable today: the Greengrass component name, not derivable from the token.
            greengrass_name: entry
                .get("greengrassComponentName")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            // Not resolvable yet: the digest and the per-version config schema both come from the
            // release index, which no component publishes (RM-013). Leaving them absent is what
            // makes the lock's `unresolved` reason truthful.
            digest: None,
            config_schema: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_tools_map_to_their_binaries() {
        assert_eq!(ExternalTool::Gdk.binary(), "gdk");
        assert_eq!(ExternalTool::GreengrassCli.binary(), "greengrass-cli");
    }

    #[test]
    fn which_finds_a_tool_that_certainly_exists() {
        // `cargo` is running this test, so it is on PATH by construction.
        assert!(which("cargo").is_some());
    }

    #[test]
    fn which_reports_absence_rather_than_guessing() {
        assert!(which("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn require_names_the_missing_tool() {
        // Use a tool we know is absent by pointing at a binary that cannot exist.
        let err = which("definitely-not-a-real-binary-xyzzy");
        assert!(err.is_none());
        let e = require(ExternalTool::Gdk);
        // gdk may or may not be installed here; the contract under test is only that a
        // failure names the tool rather than saying "something went wrong".
        if let Err(PortError::Unavailable(msg)) = e {
            assert!(msg.contains("gdk"), "{msg}");
        }
    }

    use ec_deploy::ports::TargetsPort;

    // --- lock loading ---------------------------------------------------------------------

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn a_lock_is_named_after_its_definition_and_sits_beside_it() {
        let p = lock_path_for(Path::new("sites/dallas/site.yaml"));
        assert_eq!(p, Path::new("sites/dallas/site.lock"));
        // A definition with no stem still resolves somewhere predictable rather than panicking.
        assert_eq!(lock_path_for(Path::new("")), Path::new("./deployment.lock"));
    }

    #[test]
    fn an_absent_lock_is_fine_but_an_unreadable_one_is_not() {
        let d = tempfile::tempdir().unwrap();
        let def = write(d.path(), "site.yaml", "");
        assert!(
            read_lock(&def).unwrap().is_none(),
            "locking is a separate step; not having run it is not an error"
        );

        write(d.path(), "site.lock", "{ not json");
        let e = read_lock(&def).unwrap_err();
        assert!(e.contains("readable lock"), "{e}");
    }

    #[test]
    fn a_lock_from_a_future_version_is_refused_rather_than_misread() {
        let d = tempfile::tempdir().unwrap();
        let def = write(d.path(), "site.yaml", "");
        write(
            d.path(),
            "site.lock",
            r#"{"lockVersion": 99, "definition": "site.yaml", "components": []}"#,
        );
        let e = read_lock(&def).unwrap_err();
        assert!(e.contains("lock version 99"), "{e}");
        assert!(
            e.contains("deployment lock"),
            "the error must say how to fix it: {e}"
        );
    }

    #[test]
    fn a_readable_lock_comes_back_whole() {
        let d = tempfile::tempdir().unwrap();
        let def = write(d.path(), "site.yaml", "");
        write(
            d.path(),
            "site.lock",
            r#"{"lockVersion": 1, "definition": "site.yaml", "components": [
                 {"component": "opcua-adapter", "version": "1.0.0",
                  "greengrassName": "com.mbreissi.edgecommons.OpcUaAdapter"}]}"#,
        );
        let lock = read_lock(&def).unwrap().expect("present");
        assert_eq!(
            lock.lookup("opcua-adapter")
                .unwrap()
                .greengrass_name
                .as_deref(),
            Some("com.mbreissi.edgecommons.OpcUaAdapter")
        );
    }

    // --- catalog loading ------------------------------------------------------------------

    #[test]
    fn a_catalog_loads_from_a_local_path_so_locking_is_testable_offline() {
        let d = tempfile::tempdir().unwrap();
        let path = write(
            d.path(),
            "components.json",
            r#"{"components": [{"name": "modbus-adapter",
                                "greengrassComponentName": "com.mbreissi.edgecommons.ModbusAdapter"}]}"#,
        );
        let catalog = load_catalog(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(catalog["components"][0]["name"], "modbus-adapter");
    }

    #[test]
    fn a_catalog_source_that_cannot_be_read_says_which_kind_of_problem_it_is() {
        // Distinguishing these matters: one is "you pointed at nothing", the other is "this
        // build cannot do that", and they call for different fixes.
        let e = load_catalog(Some("https://example.invalid/components.json")).unwrap_err();
        assert!(matches!(e, PortError::Unavailable(_)), "{e:?}");
        let e = load_catalog(Some("no/such/catalog.json")).unwrap_err();
        assert!(matches!(e, PortError::NotFound(_)), "{e:?}");

        let d = tempfile::tempdir().unwrap();
        let path = write(d.path(), "components.json", "{ not json");
        let e = load_catalog(Some(path.to_str().unwrap())).unwrap_err();
        assert!(matches!(e, PortError::Other(_)), "{e:?}");
    }

    // --- pin resolution -------------------------------------------------------------------

    fn targets(catalog: &str) -> RegistryTargets {
        RegistryTargets {
            catalog: serde_json::from_str(catalog).unwrap(),
        }
    }

    fn pin(component: &str) -> ec_deploy::Pin {
        ec_deploy::Pin {
            component: component.into(),
            version: "1.0.0".into(),
            greengrass_name: None,
            digest: None,
            config_schema: None,
        }
    }

    #[test]
    fn resolving_a_pin_supplies_the_name_and_admits_what_it_cannot_supply() {
        let t = targets(
            r#"{"components": [{"name": "opcua-adapter",
                                "greengrassComponentName": "com.mbreissi.edgecommons.OpcUaAdapter"}]}"#,
        );
        let resolved = t.resolve_pin(&pin("opcua-adapter")).unwrap();
        // The name is not derivable from the token — `opcua-adapter` publishes as `OpcUaAdapter` —
        // which is exactly why locking it is worth a network call.
        assert_eq!(
            resolved.greengrass_name.as_deref(),
            Some("com.mbreissi.edgecommons.OpcUaAdapter")
        );
        // The digest and config schema come from the release index, which nothing publishes yet.
        // Leaving them absent is what keeps the lock's `unresolved` reason truthful.
        assert!(resolved.digest.is_none());
        assert!(resolved.config_schema.is_none());
    }

    #[test]
    fn a_digest_typed_into_the_definition_is_a_claim_not_a_resolution() {
        let t = targets(r#"{"components": [{"name": "opcua-adapter"}]}"#);
        let mut declared = pin("opcua-adapter");
        declared.digest = Some("sha256:iSaidSo".into());
        let resolved = t.resolve_pin(&declared).unwrap();
        assert!(
            resolved.digest.is_none(),
            "a hand-written digest must not come back looking verified"
        );
    }

    #[test]
    fn a_component_the_registry_does_not_carry_is_reported_not_invented() {
        let t = targets(r#"{"components": []}"#);
        let e = t.resolve_pin(&pin("ghost")).unwrap_err();
        assert!(matches!(e, PortError::NotFound(_)), "{e:?}");
    }

    #[test]
    fn a_catalog_entry_without_a_greengrass_name_resolves_without_one() {
        let t = targets(r#"{"components": [{"name": "host-only"}]}"#);
        let resolved = t.resolve_pin(&pin("host-only")).unwrap();
        assert!(
            resolved.greengrass_name.is_none(),
            "a HOST-only component has no Greengrass name to invent"
        );
    }
}
