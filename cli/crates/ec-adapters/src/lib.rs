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

use std::path::PathBuf;

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
