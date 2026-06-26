//! # Parameter sources (the pluggable seam)
//!
//! **One-liner purpose**: A [`ParameterSource`] is the backend the parameter service reads from —
//! AWS SSM (cloud), a mounted directory (K8s ConfigMap/Secret volumes, Docker secrets), env vars,
//! or a custom host-supplied source. The service (cache, refresh, typed reads) is identical
//! regardless of source.

use std::path::PathBuf;

use crate::error::GgError;
use crate::Result;

/// A parameter value fetched from a source. `secure` values (SSM SecureString, a `mountedDir`
/// secret path, …) must never be logged.
#[derive(Debug, Clone)]
pub struct ParamValue {
    /// Raw value bytes (UTF-8 for SSM / env / text files).
    pub value: Vec<u8>,
    /// Whether this value is sensitive (don't log; cache encrypted).
    pub secure: bool,
    /// Upstream version, for change detection on refresh (`None` if the source has none).
    pub version: Option<String>,
}

impl ParamValue {
    /// Construct a non-secure value.
    pub fn plain(value: impl Into<Vec<u8>>) -> Self {
        Self { value: value.into(), secure: false, version: None }
    }
}

/// The pluggable parameter backend. Implementations must be `Send + Sync`.
pub trait ParameterSource: Send + Sync {
    /// Fetch one parameter by name, or `None` if it does not exist.
    fn fetch(&self, name: &str) -> Result<Option<ParamValue>>;

    /// Fetch every parameter under `path` (recursively when `recursive`). Empty when absent.
    fn fetch_by_path(&self, path: &str, recursive: bool) -> Result<Vec<(String, ParamValue)>>;

    /// Stable id for diagnostics/stats (e.g. `"awsSsm"`, `"mountedDir"`, `"env"`).
    fn source_id(&self) -> &str;
}

// ---------------------------------------------------------------------------
// EnvSource — parameters from environment variables (containers / dev / STANDALONE).
// ---------------------------------------------------------------------------

/// Reads parameters from environment variables under a prefix. A name `/myapp/db/host` maps to the
/// env var `<PREFIX>MYAPP_DB_HOST` and back. Values are treated as non-secure (env is plaintext).
pub struct EnvSource {
    prefix: String,
}

impl EnvSource {
    /// New source reading vars under `prefix` (e.g. `"GG_PARAM_"`).
    pub fn new(prefix: impl Into<String>) -> Self {
        Self { prefix: prefix.into() }
    }

    /// Map a parameter name to its env-var name.
    fn to_env(&self, name: &str) -> String {
        let body: String = name
            .trim_start_matches('/')
            .chars()
            .map(|c| match c {
                '/' | '-' | '.' => '_',
                c => c.to_ascii_uppercase(),
            })
            .collect();
        format!("{}{}", self.prefix, body)
    }

    /// Map an env-var name back to a parameter name (lossy: `_` -> `/`).
    fn from_env(&self, var: &str) -> Option<String> {
        var.strip_prefix(&self.prefix)
            .map(|rest| format!("/{}", rest.to_ascii_lowercase().replace('_', "/")))
    }
}

impl ParameterSource for EnvSource {
    fn fetch(&self, name: &str) -> Result<Option<ParamValue>> {
        Ok(std::env::var(self.to_env(name)).ok().map(|v| ParamValue::plain(v.into_bytes())))
    }

    fn fetch_by_path(&self, path: &str, _recursive: bool) -> Result<Vec<(String, ParamValue)>> {
        let mut out = Vec::new();
        for (k, v) in std::env::vars() {
            if let Some(name) = self.from_env(&k) {
                if name.starts_with(path) {
                    out.push((name, ParamValue::plain(v.into_bytes())));
                }
            }
        }
        Ok(out)
    }

    fn source_id(&self) -> &str {
        "env"
    }
}

// ---------------------------------------------------------------------------
// MountedDirSource — parameters from a directory tree (K8s ConfigMap/Secret volumes,
// Docker secrets at /run/secrets, bare config dirs). No API client / RBAC needed.
// ---------------------------------------------------------------------------

/// Reads parameters from files under a root directory: a file at `<root>/myapp/db/host` is the
/// parameter `/myapp/db/host` with the file's bytes as its value. Files whose parameter name falls
/// under one of `secure_paths` are flagged `secure` (a K8s Secret volume vs a ConfigMap volume).
pub struct MountedDirSource {
    root: PathBuf,
    secure_paths: Vec<String>,
}

impl MountedDirSource {
    /// New source rooted at `root`; parameters under any `secure_paths` prefix are sensitive.
    pub fn new(root: impl Into<PathBuf>, secure_paths: Vec<String>) -> Self {
        Self { root: root.into(), secure_paths }
    }

    fn is_secure(&self, name: &str) -> bool {
        self.secure_paths.iter().any(|p| name.starts_with(p.as_str()))
    }

    fn name_to_path(&self, name: &str) -> PathBuf {
        self.root.join(name.trim_start_matches('/'))
    }

    /// Recursively collect files under `dir` into `out`, keyed by parameter name (relative to root,
    /// `/`-separated). Skips dotfiles/dirs — K8s projects volumes with internal `..data` /
    /// `..2025_…` symlinked entries that must not be surfaced as parameters.
    fn walk(&self, dir: &std::path::Path, recursive: bool, out: &mut Vec<(String, ParamValue)>) -> Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(GgError::Parameters(format!("read dir {}: {e}", dir.display()))),
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let fname = file_name.to_string_lossy();
            // Shared dotfile filter (single source of truth in the config CONFIGMAP source) — skips
            // the kubelet symlink farm (..data, ..2025_..., ..data_tmp) and hidden entries.
            if crate::config::source::configmap::is_projection_artifact(&fname) {
                continue; // K8s internal (..data, ..2025_...) / hidden
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let path = entry.path();
            if ft.is_dir() {
                if recursive {
                    self.walk(&path, recursive, out)?;
                }
            } else {
                // Parameter name = "/" + path relative to root, with platform separators normalized.
                let rel = path.strip_prefix(&self.root).unwrap_or(&path);
                let name = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
                let value = std::fs::read(&path)
                    .map_err(|e| GgError::Parameters(format!("read {}: {e}", path.display())))?;
                let secure = self.is_secure(&name);
                out.push((name, ParamValue { value, secure, version: None }));
            }
        }
        Ok(())
    }
}

impl ParameterSource for MountedDirSource {
    fn fetch(&self, name: &str) -> Result<Option<ParamValue>> {
        let path = self.name_to_path(name);
        match std::fs::read(&path) {
            Ok(value) => Ok(Some(ParamValue { value, secure: self.is_secure(name), version: None })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            // A directory (not a file) at that name is "not a parameter".
            Err(e) if matches!(e.raw_os_error(), Some(21)) => Ok(None),
            Err(e) => Err(GgError::Parameters(format!("read {}: {e}", path.display()))),
        }
    }

    fn fetch_by_path(&self, path: &str, recursive: bool) -> Result<Vec<(String, ParamValue)>> {
        let base = self.name_to_path(path);
        let mut out = Vec::new();
        self.walk(&base, recursive, &mut out)?;
        Ok(out)
    }

    fn source_id(&self) -> &str {
        "mountedDir"
    }
}
