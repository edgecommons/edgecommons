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
}

/// Load a definition (file path) and every file it references. This is the I/O half the
/// kernel refuses to do: the kernel names the paths, the adapter reads them.
pub fn load_workspace(definition: &Path) -> Result<LoadedWorkspace, String> {
    let definition_text = std::fs::read_to_string(definition)
        .map_err(|e| format!("reading {}: {e}", definition.display()))?;
    let doc = ec_deploy::workspace::parse_definition(&definition_text).map_err(|e| e.to_string())?;
    let root = definition
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut files = std::collections::BTreeMap::new();
    for rel in ec_deploy::workspace::referenced_paths(&doc) {
        let path = root.join(&rel);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {} (referenced by the definition): {e}", path.display()))?;
        files.insert(rel, text);
    }
    Ok(LoadedWorkspace {
        workspace: ec_deploy::workspace::Workspace { definition: doc, files },
        root,
        definition_text,
    })
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
    Some(if dirty { format!("{commit}-dirty") } else { commit })
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
}
