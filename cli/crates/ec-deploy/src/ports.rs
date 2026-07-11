//! The five ports (DESIGN-cli §8.2).
//!
//! The rule that keeps this honest, and that belongs in code review rather than prose:
//! **no cloud SDK may be linked above the port boundary.** The Greengrass adapter may
//! depend on the AWS SDK; nothing else in the system may. This crate depends on neither
//! an HTTP client nor a cloud SDK, and it must stay that way — that is what prevents the
//! tool's Greengrass ancestry from quietly reasserting itself as an AWS product.
//!
//! Each port has a zero-cost local adapter (`ec-adapters`), which is what makes local
//! development free and air-gapped operation ordinary rather than a special mode.

use std::path::PathBuf;

use crate::{Pin, Plan};

/// Source of truth: definitions, layers, releases, evidence, approvals.
///
/// Local adapter: a plain local clone. Production: any Git host.
pub trait GitPort {
    /// Read a path at a ref. `None` when the path does not exist at that ref.
    fn read_at(&self, git_ref: &str, path: &str) -> Result<Option<Vec<u8>>, PortError>;
    /// The commit the working tree is at — an input to every render hash (§8.3).
    fn head_commit(&self) -> Result<String, PortError>;
}

/// Who authored, who approved, who may edit which layer.
///
/// Local adapter: static dev users. Production: OIDC only — never a provider-specific identity.
pub trait IdentityPort {
    fn current_actor(&self) -> Result<String, PortError>;
}

/// Component artifacts, evidence bundles, release render snapshots.
///
/// Local adapter: the filesystem. Production: any **S3-compatible API** — not "S3 the service".
pub trait BlobPort {
    fn get(&self, key: &str) -> Result<Vec<u8>, PortError>;
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), PortError>;
}

/// Executes an apply. **Holds the target credentials; the Studio never does.**
///
/// This is the boundary that D-CLI-8 and D-CLI-10 both rest on: deterministic,
/// credential-free work belongs in the CLI; anything that needs a credential or mutates
/// the world belongs behind this port. Local adapter: a subprocess.
pub trait RunnerPort {
    fn run_apply(&self, plan: &Plan) -> Result<ApplyReport, PortError>;
}

/// The three control planes the renderers speak to.
///
/// Local adapter: `kind`, `greengrass-cli` local deployment, supervisord in containers.
pub trait TargetsPort {
    /// Resolve a pinned component version to an immutable digest, and fetch the config
    /// schema that version publishes (D-CLI-16). This is the **one** networked operation
    /// in the whole binary, and it happens only in `deployment lock` (§8.7).
    fn resolve_pin(&self, pin: &Pin) -> Result<Pin, PortError>;
}

/// The per-node outcome of an apply. Greengrass deploys **per thing**, so failure is
/// per-node rather than all-or-nothing across a group (REVIEW #3).
#[derive(Debug, Clone, Default)]
pub struct ApplyReport {
    pub applied: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl ApplyReport {
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// A port failure. Deliberately transport-agnostic: the kernel must not learn what a
/// 404, an `AccessDenied`, or a `git` exit status is.
#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("denied: {0}")]
    Denied(String),
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Other(String),
}

/// Where a local adapter roots itself.
#[derive(Debug, Clone)]
pub struct LocalRoot(pub PathBuf);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_apply_is_not_complete() {
        let mut r = ApplyReport { applied: vec!["gw-fill-01".into()], failed: vec![] };
        assert!(r.is_complete());
        r.failed.push(("gw-pack-02".into(), "unreachable".into()));
        assert!(!r.is_complete());
    }
}
