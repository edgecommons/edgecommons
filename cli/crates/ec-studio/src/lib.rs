//! The Deployment Studio server shell (DESIGN-cli §8.4, RM-012).
//!
//! *"The CLI is the product; the server is a shell around it."* The server adds only
//! branch and draft orchestration, the UI, evidence correlation, and access control —
//! it adds no capability that the CLI does not already have. That is why the first
//! slices need no hosting decision to be paid for, and why `studio serve` can be a seam
//! rather than a dependency.
//!
//! # Status
//!
//! A compiling seam. The `axum` server and the embedded SPA are later work. **Nothing in
//! `ec-deploy` may assume a server exists** — if that invariant ever breaks, the CLI stops
//! working offline, which is the property RM-012 exists to protect.

use ec_diag::Fatal;

/// Where the server would serve from, and what it would serve against.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// The Git repository holding desired state. Git is the database; SQLite is a
    /// rebuildable cache; there is no third datastore.
    pub repo: String,
    pub bind: String,
}

/// Serve the Studio UI over the same kernel the CLI uses.
///
/// # Errors
///
/// Always returns [`Fatal::NotImplemented`] in this build, so that invoking the verb
/// tells the truth rather than failing obscurely.
pub fn serve(_opts: &ServeOptions) -> Result<(), Fatal> {
    Err(Fatal::NotImplemented(
        "`studio serve` is not implemented in this build. The kernel it wraps is Phase P4; \
         see docs/platform/DESIGN-cli.md §8.4."
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_reports_not_implemented_rather_than_failing_obscurely() {
        let e = serve(&ServeOptions { repo: ".".into(), bind: "127.0.0.1:8080".into() }).unwrap_err();
        assert_eq!(e.exit_code(), ec_diag::ExitCode::NotImplemented);
        assert!(e.to_string().contains("DESIGN-cli.md"));
    }
}
