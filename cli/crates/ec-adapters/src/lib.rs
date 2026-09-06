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

/// The result of loading a deployment workspace from the filesystem: the parsed authored
/// definition (one topology + per-platform profiles) plus every referenced file's text, rooted at
/// the definition's directory. A command picks a profile and calls [`LoadedWorkspace::workspace`]
/// to get the flat, effective workspace the kernel renders.
pub struct LoadedWorkspace {
    pub authored: ec_deploy::model::AuthoredDefinition,
    pub files: std::collections::BTreeMap<String, String>,
    pub root: PathBuf,
    /// The definition file itself (e.g. `<root>/definition.yaml`) — kept so callers that need to
    /// resolve its ownership or commit history do not have to reconstruct the name.
    pub definition_path: PathBuf,
    pub definition_text: String,
    /// The lock committed beside the definition, when `deployment lock` has been run. It supplies
    /// what the definition itself need not carry — today the Greengrass component name (§8.7).
    pub lock: Option<ec_deploy::lock::LockFile>,
}

impl LoadedWorkspace {
    /// The effective workspace for one profile — the topology merged with that profile's delivery.
    pub fn workspace(&self, profile: &str) -> Result<ec_deploy::workspace::Workspace, String> {
        let definition = self
            .authored
            .effective(profile)
            .map_err(|e| e.to_string())?;
        Ok(ec_deploy::workspace::Workspace {
            definition,
            files: self.files.clone(),
        })
    }

    /// Every profile name, in authored order.
    #[must_use]
    pub fn profile_names(&self) -> Vec<String> {
        self.authored.profile_names()
    }

    /// The single profile whose `family` matches (e.g. `"HOST"`), or an error naming the choices.
    pub fn profile_for_family(&self, family: &str) -> Result<String, String> {
        let matches: Vec<&String> = self
            .authored
            .profiles
            .iter()
            .filter(|(_, p)| p.family == family)
            .map(|(name, _)| name)
            .collect();
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(format!(
                "no profile targets {family}; the definition has: {}",
                self.profile_names().join(", ")
            )),
            many => Err(format!(
                "{} profiles target {family} ({}); select one with --profile",
                many.len(),
                many.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// Load a definition (file path) and every file it references. This is the I/O half the
/// kernel refuses to do: the kernel names the paths, the adapter reads them.
pub fn load_workspace(definition: &Path) -> Result<LoadedWorkspace, String> {
    let definition_text = std::fs::read_to_string(definition)
        .map_err(|e| format!("reading {}: {e}", definition.display()))?;
    let authored =
        ec_deploy::workspace::parse_authored(&definition_text).map_err(|e| e.to_string())?;
    let root = definition
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut files = std::collections::BTreeMap::new();
    for rel in ec_deploy::workspace::referenced_paths_authored(&authored) {
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
        authored,
        files,
        root,
        definition_path: definition.to_path_buf(),
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

/// The absolute path of the Git repository containing `dir` (its `--show-toplevel`), or `None`
/// outside a repo. CODEOWNERS patterns are anchored at this root, so the Studio resolves ownership
/// against repo-relative paths, not paths relative to the definition's directory.
#[must_use]
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let top = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

/// The repository's `CODEOWNERS` — its repo-relative display path and full text — searched in the
/// three locations Git hosts honor (`CODEOWNERS`, `.github/CODEOWNERS`, `docs/CODEOWNERS`), in that
/// order. `None` means the repository defines no ownership file, which the Studio renders as
/// "changes fall to the default branch-protection review" — never as "unrestricted".
#[must_use]
pub fn read_codeowners(dir: &Path) -> Option<(String, String)> {
    let root = repo_root(dir)?;
    for rel in ["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"] {
        let path = root.join(rel);
        if let Ok(text) = std::fs::read_to_string(&path) {
            return Some((rel.to_string(), text));
        }
    }
    None
}

/// A file's path relative to the Git repo root (forward-slashed), for matching against CODEOWNERS.
/// Falls back to the path as given when it is not under a repo or the prefix cannot be stripped.
#[must_use]
pub fn repo_relative(dir: &Path, file: &Path) -> String {
    let slashed = |p: &Path| p.to_string_lossy().replace('\\', "/");
    match repo_root(dir) {
        Some(root) => file
            .strip_prefix(&root)
            .map(slashed)
            .unwrap_or_else(|_| slashed(file)),
        None => slashed(file),
    }
}

// --- The Git read + draft-write ports, backed by a local clone --------------------------------

/// The committer identity the Studio writes as when it acts as itself (register #16: ship a bot/App
/// identity behind the same port as per-user acting-as-user OAuth). A later slice sets the *author* to
/// the real actor from the identity port; the committer stays the Studio.
const STUDIO_NAME: &str = "Deployment Studio";
const STUDIO_EMAIL: &str = "studio@edgecommons.local";

impl LocalGit {
    fn git(&self, args: &[&str]) -> Result<std::process::Output, PortError> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&self.root.0)
            .args(args)
            .output()
            .map_err(|e| PortError::Unavailable(format!("git: {e}")))
    }

    fn git_ok(&self, args: &[&str]) -> Result<String, PortError> {
        let out = self.git(args)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(PortError::Other(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
}

impl ec_deploy::ports::GitPort for LocalGit {
    fn read_at(&self, git_ref: &str, path: &str) -> Result<Option<Vec<u8>>, PortError> {
        // `git show <ref>:<path>` accepts any tree-ish, including the tree OID a merge-tree yields —
        // which is how the render pipeline reads the would-actually-deploy output of a merge.
        let out = self.git(&["show", &format!("{git_ref}:{path}")])?;
        if out.status.success() {
            Ok(Some(out.stdout))
        } else {
            // A missing path at a valid ref is the common, non-error case (a layer not yet authored).
            Ok(None)
        }
    }

    fn head_commit(&self) -> Result<String, PortError> {
        self.git_ok(&["rev-parse", "HEAD"])
    }
}

impl ec_deploy::ports::DraftPort for LocalGit {
    fn open(&self, git_ref: &str, base_ref: &str) -> Result<(), PortError> {
        // Idempotent: opening an existing draft is a no-op (propose is not "recreate").
        if self
            .git(&["rev-parse", "--verify", "--quiet", git_ref])?
            .status
            .success()
        {
            return Ok(());
        }
        self.git_ok(&["branch", git_ref, base_ref]).map(|_| ())
    }

    fn write_file(
        &self,
        git_ref: &str,
        path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<(), PortError> {
        // Commit onto the draft branch **without a working tree**: hash the blob, build a tree from
        // the branch's tree with the path updated in a throwaway index, commit-tree, move the ref.
        // This lets the server write a draft while the read side keeps serving the working tree.
        let blob = {
            use std::io::Write;
            let mut child = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root.0)
                .args(["hash-object", "-w", "--stdin"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| PortError::Unavailable(format!("git: {e}")))?;
            child
                .stdin
                .take()
                .ok_or_else(|| PortError::Other("git stdin".into()))?
                .write_all(bytes)
                .map_err(|e| PortError::Other(format!("writing blob: {e}")))?;
            let out = child
                .wait_with_output()
                .map_err(|e| PortError::Unavailable(format!("git: {e}")))?;
            if !out.status.success() {
                return Err(PortError::Other(
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        let index = self.root.0.join(".git").join(format!(
            "studio-index-{}",
            git_ref.replace(['/', '\\'], "_")
        ));
        let index_env = index.to_string_lossy().to_string();
        let run_indexed = |args: &[&str]| -> Result<String, PortError> {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root.0)
                .env("GIT_INDEX_FILE", &index_env)
                .args(args)
                .output()
                .map_err(|e| PortError::Unavailable(format!("git: {e}")))?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err(PortError::Other(
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ))
            }
        };
        run_indexed(&["read-tree", git_ref])?;
        run_indexed(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},{path}"),
        ])?;
        let tree = run_indexed(&["write-tree"])?;
        let _ = std::fs::remove_file(&index);

        let commit = {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root.0)
                .env("GIT_AUTHOR_NAME", STUDIO_NAME)
                .env("GIT_AUTHOR_EMAIL", STUDIO_EMAIL)
                .env("GIT_COMMITTER_NAME", STUDIO_NAME)
                .env("GIT_COMMITTER_EMAIL", STUDIO_EMAIL)
                .args(["commit-tree", &tree, "-p", git_ref, "-m", message])
                .output()
                .map_err(|e| PortError::Unavailable(format!("git: {e}")))?;
            if !out.status.success() {
                return Err(PortError::Other(
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        self.git_ok(&["update-ref", &format!("refs/heads/{git_ref}"), &commit])
            .map(|_| ())
    }

    fn list(&self) -> Result<Vec<String>, PortError> {
        let out = self.git_ok(&[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/draft/",
        ])?;
        Ok(out
            .lines()
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .collect())
    }

    fn merge_tree(
        &self,
        draft_ref: &str,
        main_ref: &str,
    ) -> Result<ec_deploy::ports::MergeResult, PortError> {
        // Merge in a **temporary detached worktree** rather than `merge-tree --write-tree` (which needs
        // git ≥ 2.38). A linked worktree shares the object store, so the tree we write is readable from
        // the main clone, and the main working tree the read-only server serves is never touched. This
        // works on every git that has worktrees (2.5+).
        let wt = self.root.0.join(".git").join(format!(
            "studio-merge-{}",
            draft_ref.replace(['/', '\\'], "_")
        ));
        let wt_str = wt.to_string_lossy().to_string();
        // Start clean: drop any worktree left by an aborted prior run.
        let _ = self.git(&["worktree", "remove", "--force", &wt_str]);
        let _ = std::fs::remove_dir_all(&wt);
        self.git_ok(&["worktree", "add", "--detach", &wt_str, draft_ref])?;

        let in_wt = |args: &[&str]| -> Result<std::process::Output, PortError> {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&wt)
                .env("GIT_AUTHOR_NAME", STUDIO_NAME)
                .env("GIT_AUTHOR_EMAIL", STUDIO_EMAIL)
                .env("GIT_COMMITTER_NAME", STUDIO_NAME)
                .env("GIT_COMMITTER_EMAIL", STUDIO_EMAIL)
                .args(args)
                .output()
                .map_err(|e| PortError::Unavailable(format!("git: {e}")))
        };

        let merge = in_wt(&["merge", "--no-commit", "--no-ff", main_ref])?;
        let result = if merge.status.success() {
            // The merged result is staged; the tree it writes lands in the shared object store.
            let tree = in_wt(&["write-tree"])?;
            if tree.status.success() {
                ec_deploy::ports::MergeResult::Clean(
                    String::from_utf8_lossy(&tree.stdout).trim().to_string(),
                )
            } else {
                ec_deploy::ports::MergeResult::Textual(vec![
                    "could not write the merged tree".to_string(),
                ])
            }
        } else {
            let u = in_wt(&["diff", "--name-only", "--diff-filter=U"])?;
            let mut paths: Vec<String> = String::from_utf8_lossy(&u.stdout)
                .lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if paths.is_empty() {
                paths.push("merge failed".to_string());
            }
            ec_deploy::ports::MergeResult::Textual(paths)
        };

        let _ = self.git(&["worktree", "remove", "--force", &wt_str]);
        let _ = std::fs::remove_dir_all(&wt);
        Ok(result)
    }
}

/// The merge-base of a draft and current main — where the draft started, the `base` render point.
#[must_use]
pub fn merge_base(git: &LocalGit, draft_ref: &str, main_ref: &str) -> Option<String> {
    git.git_ok(&["merge-base", main_ref, draft_ref]).ok()
}

/// The definition-relative files a draft changes since it branched from `main_ref`. Files outside
/// the definition's directory are dropped, so the result is the set of layers/bindings the draft
/// touches — what the UI intersects with a scope to compute advisory presence (register #16 ruling 6).
#[must_use]
pub fn draft_changed_files(
    git: &LocalGit,
    dir_prefix: &str,
    draft_ref: &str,
    main_ref: &str,
) -> Vec<String> {
    let out = git
        .git_ok(&["diff", "--name-only", &format!("{main_ref}...{draft_ref}")])
        .unwrap_or_default();
    out.lines()
        .filter_map(|line| {
            let path = line.trim();
            if path.is_empty() {
                return None;
            }
            if dir_prefix.is_empty() {
                Some(path.to_string())
            } else {
                path.strip_prefix(&format!("{}/", dir_prefix.trim_end_matches('/')))
                    .map(str::to_string)
            }
        })
        .collect()
}

/// A Git remote broken into its host and project path — host-agnostic, so the same adapter drives
/// GitHub, GitHub Enterprise, GitLab (incl. self-hosted and subgroups), and others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    /// The host, e.g. `github.com`, `github.acme.com`, `gitlab.com`, `git.internal`.
    pub host: String,
    /// The project path, e.g. `edgecommons/bottling-company-test` (GitLab may nest: `grp/sub/repo`).
    pub path: String,
}

/// The kind of Git host, which decides the review-request URL scheme, CLI, and vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// GitHub.com or GitHub Enterprise — pull requests, driven by `gh`.
    GitHub,
    /// GitLab.com or self-hosted GitLab — merge requests, driven by `glab`.
    GitLab,
    /// A host we do not recognise: push works, but the review request is opened by the operator.
    Other,
}

impl HostKind {
    /// The host's own term for a review request — shown in the UI so the word matches the host.
    #[must_use]
    pub fn review_term(self) -> &'static str {
        match self {
            HostKind::GitHub => "pull request",
            HostKind::GitLab => "merge request",
            HostKind::Other => "review request",
        }
    }
}

/// Classify a host. Known SaaS domains are exact; self-hosted is inferred from the host name
/// (`github.*`/GHE, `*gitlab*`), and anything else is `Other` — pushed, but opened by the operator.
#[must_use]
pub fn host_kind(host: &str) -> HostKind {
    let h = host.to_ascii_lowercase();
    if h == "github.com" || h.starts_with("github.") || h.contains("github") {
        HostKind::GitHub
    } else if h == "gitlab.com" || h.starts_with("gitlab.") || h.contains("gitlab") {
        HostKind::GitLab
    } else {
        HostKind::Other
    }
}

/// Parse any Git remote URL (https, `git@host:path`, `ssh://git@host/path`) into host + project
/// path. Not host-specific — the point of the port is that the host is pluggable.
#[must_use]
pub fn parse_remote(url: &str) -> Option<RemoteInfo> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
    {
        // Drop any `user@` before the host, then split host / path on the first slash.
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        rest.split_once('/')?
    } else {
        // scp-style: `git@host:owner/repo`.
        url.strip_prefix("git@")?.split_once(':')?
    };
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    if host.is_empty() || path.is_empty() || !path.contains('/') {
        return None;
    }
    Some(RemoteInfo {
        host: host.to_string(),
        path: path.to_string(),
    })
}

/// The `origin` remote parsed to host + path, or `None` for a purely local clone.
#[must_use]
pub fn origin_remote(git: &LocalGit) -> Option<RemoteInfo> {
    parse_remote(&git.git_ok(&["remote", "get-url", "origin"]).ok()?)
}

/// The host's web page to open a review request for `branch` — pull-request or merge-request create
/// page depending on the host. `None` for an unrecognised host (the operator opens it themselves).
#[must_use]
pub fn review_page_url(remote: &RemoteInfo, branch: &str) -> Option<String> {
    match host_kind(&remote.host) {
        HostKind::GitHub => Some(format!(
            "https://{}/{}/pull/new/{branch}",
            remote.host, remote.path
        )),
        HostKind::GitLab => Some(format!(
            "https://{}/{}/-/merge_requests/new?merge_request%5Bsource_branch%5D={branch}",
            remote.host, remote.path
        )),
        HostKind::Other => None,
    }
}

/// The CLI arguments to open a review request, per host CLI — pure, so testable without running them.
/// `gh pr create` / `glab mr create`; the body names CODEOWNERS (or the host's approval rules) as the
/// gate. `None` for a host with no supported CLI.
#[must_use]
pub fn review_create_args(
    kind: HostKind,
    git_ref: &str,
    base: &str,
    title: &str,
) -> Option<Vec<String>> {
    let body = "Proposed via Deployment Studio. Review and merge here — the host's code owners gate the merge.";
    let args: Vec<&str> = match kind {
        HostKind::GitHub => vec![
            "pr", "create", "--head", git_ref, "--base", base, "--title", title, "--body", body,
        ],
        HostKind::GitLab => vec![
            "mr",
            "create",
            "--source-branch",
            git_ref,
            "--target-branch",
            base,
            "--title",
            title,
            "--description",
            body,
            "--yes",
        ],
        HostKind::Other => return None,
    };
    Some(args.into_iter().map(str::to_string).collect())
}

fn host_cli(kind: HostKind) -> Option<&'static str> {
    match kind {
        HostKind::GitHub => Some("gh"),
        HostKind::GitLab => Some("glab"),
        HostKind::Other => None,
    }
}

impl ec_deploy::ports::HostPort for LocalGit {
    fn available(&self) -> bool {
        // Apply needs somewhere to push. Any remote qualifies: for a known host we open the review
        // request, for an unknown one we push and hand off. No remote → apply is unavailable.
        origin_remote(self).is_some()
    }

    fn push_draft(&self, git_ref: &str) -> Result<(), PortError> {
        // Uses the operator's git credential helper — the Studio holds no token of its own.
        self.git_ok(&["push", "-u", "origin", git_ref]).map(|_| ())
    }

    fn open_review_request(
        &self,
        git_ref: &str,
        base: &str,
        title: &str,
    ) -> Result<ec_deploy::ports::ReviewSubmission, PortError> {
        let remote =
            origin_remote(self).ok_or_else(|| PortError::Unavailable("no origin remote".into()))?;
        let kind = host_kind(&remote.host);
        let term = kind.review_term().to_string();
        let page = review_page_url(&remote, git_ref);

        // If the host's CLI is present, open the request as the authenticated user; the host enforces
        // their permissions and code-owner rules. The Studio never merges — it only opens the request.
        if let (Some(cli), Some(args)) = (
            host_cli(kind).filter(|c| which(c).is_some()),
            review_create_args(kind, git_ref, base, title),
        ) {
            let out = std::process::Command::new(cli)
                .current_dir(&self.root.0)
                .args(&args)
                .output()
                .map_err(|e| PortError::Unavailable(format!("{cli}: {e}")))?;
            if out.status.success() {
                if let Some(url) = String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .find(|t| t.starts_with("https://"))
                {
                    return Ok(ec_deploy::ports::ReviewSubmission {
                        url: Some(url.to_string()),
                        term,
                        opened: true,
                    });
                }
            }
            // CLI present but the create failed (e.g. not authenticated): fall through to the page,
            // so the operator can finish it — the branch is already pushed.
        }
        Ok(ec_deploy::ports::ReviewSubmission {
            url: page,
            term,
            opened: false,
        })
    }
}

/// The branch the working tree is on — the default target for a draft's review request.
#[must_use]
pub fn current_branch(git: &LocalGit) -> Option<String> {
    git.git_ok(&["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|b| b != "HEAD")
}

/// Render a definition **at a Git ref** (a commit or a tree-ish, so a merge-tree OID works). The
/// definition lives at `<dir_prefix>/definition.yaml`; its referenced layers and bindings are read at
/// the same ref, so this is a pure function of the tree — the whole reason semantic conflict detection
/// is possible (register #16). `dir_prefix` is the definition's directory relative to the repo root
/// (`""` when the repo root is the site).
pub fn render_at_ref(
    git: &LocalGit,
    dir_prefix: &str,
    git_ref: &str,
    profile: &str,
) -> Result<ec_deploy::render::RenderOutput, String> {
    use ec_deploy::ports::GitPort;
    let at = |rel: &str| -> String {
        if dir_prefix.is_empty() {
            rel.to_string()
        } else {
            format!("{}/{rel}", dir_prefix.trim_end_matches('/'))
        }
    };
    let def_bytes = git
        .read_at(git_ref, &at("definition.yaml"))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no definition.yaml at {git_ref}"))?;
    let def_text = String::from_utf8(def_bytes).map_err(|e| e.to_string())?;
    let authored = ec_deploy::workspace::parse_authored(&def_text).map_err(|e| e.to_string())?;

    let mut files = std::collections::BTreeMap::new();
    for rel in ec_deploy::workspace::referenced_paths_authored(&authored) {
        let bytes = git
            .read_at(git_ref, &at(&rel))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{rel} missing at {git_ref}"))?;
        files.insert(
            rel.clone(),
            String::from_utf8(bytes).map_err(|e| e.to_string())?,
        );
    }

    let definition = authored.effective(profile).map_err(|e| e.to_string())?;
    let ws = ec_deploy::workspace::Workspace { definition, files };
    let env = ws
        .definition
        .environments
        .first()
        .map(|e| e.name.clone())
        .ok_or_else(|| "profile declares no environment".to_string())?;
    let target = ec_deploy::Platform::from_family(&ws.definition.target_standard.family)
        .ok_or_else(|| format!("unknown family {}", ws.definition.target_standard.family))?;
    ec_deploy::render::render(&ws, &env, target, "initial").map_err(|e| e.to_string())
}

/// The result of checking a draft for conflicts (register #16 ruling 3): first any textual merge
/// conflicts Git could not resolve, then the semantic divergences found by comparing rendered outputs.
#[derive(Debug, Clone)]
pub struct DraftCheck {
    /// Paths Git could not merge as text — a hard conflict, reported before the semantic pass.
    pub textual: Vec<String>,
    /// Semantic divergences: the merged render differs from what the author reviewed.
    pub semantic: Vec<ec_deploy::draft::Conflict>,
}

impl DraftCheck {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.textual.is_empty() && self.semantic.is_empty()
    }
}

/// Check a draft against current main for a profile, the whole engine end to end: compute the merged
/// tree, render `base` / `draft` / `main` / `merged`, and run the semantic detector.
pub fn check_draft(
    git: &LocalGit,
    dir_prefix: &str,
    profile: &str,
    draft_ref: &str,
    main_ref: &str,
) -> Result<DraftCheck, String> {
    use ec_deploy::ports::{DraftPort, MergeResult};
    let merged_tree = match git
        .merge_tree(draft_ref, main_ref)
        .map_err(|e| e.to_string())?
    {
        MergeResult::Textual(paths) => {
            return Ok(DraftCheck {
                textual: paths,
                semantic: Vec::new(),
            });
        }
        MergeResult::Clean(tree) => tree,
    };
    let base_ref = merge_base(git, draft_ref, main_ref)
        .ok_or_else(|| "no merge base between the draft and main".to_string())?;

    let base = render_at_ref(git, dir_prefix, &base_ref, profile)?;
    let draft = render_at_ref(git, dir_prefix, draft_ref, profile)?;
    let main = render_at_ref(git, dir_prefix, main_ref, profile)?;
    let merged = render_at_ref(git, dir_prefix, &merged_tree, profile)?;

    Ok(DraftCheck {
        textual: Vec::new(),
        semantic: ec_deploy::draft::detect_conflicts(&base, &draft, &main, &merged),
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
    fn remotes_parse_host_and_path_for_any_host() {
        // GitHub, GitHub Enterprise, GitLab (incl. a subgroup), https + ssh + scp forms.
        let g = |u: &str| parse_remote(u).unwrap();
        assert_eq!(
            g("https://github.com/edgecommons/edgecommons.git").host,
            "github.com"
        );
        assert_eq!(
            g("https://github.com/edgecommons/edgecommons.git").path,
            "edgecommons/edgecommons"
        );
        assert_eq!(g("git@github.com:edgecommons/bct.git").host, "github.com");
        assert_eq!(
            g("https://github.acme.internal/team/repo.git").host,
            "github.acme.internal"
        );
        assert_eq!(
            g("git@gitlab.com:group/subgroup/app.git").path,
            "group/subgroup/app"
        );
        assert_eq!(
            g("ssh://git@git.corp.example/eng/site").host,
            "git.corp.example"
        );
        // Not a project path, or unparseable: no remote invented.
        assert_eq!(parse_remote("git@github.com:onlyowner"), None);
        assert_eq!(parse_remote("/tmp/local-bare"), None);
    }

    #[test]
    fn host_kind_recognises_github_gitlab_and_self_hosted() {
        assert_eq!(host_kind("github.com"), HostKind::GitHub);
        assert_eq!(host_kind("github.acme.internal"), HostKind::GitHub); // GHE
        assert_eq!(host_kind("gitlab.com"), HostKind::GitLab);
        assert_eq!(host_kind("gitlab.corp.example"), HostKind::GitLab); // self-hosted GitLab
        assert_eq!(host_kind("bitbucket.org"), HostKind::Other);
    }

    #[test]
    fn review_urls_and_terms_are_host_specific_not_github() {
        let gh = RemoteInfo {
            host: "github.com".into(),
            path: "o/r".into(),
        };
        let ghe = RemoteInfo {
            host: "github.acme.internal".into(),
            path: "o/r".into(),
        };
        let gl = RemoteInfo {
            host: "gitlab.com".into(),
            path: "grp/sub/app".into(),
        };
        let other = RemoteInfo {
            host: "bitbucket.org".into(),
            path: "o/r".into(),
        };

        assert_eq!(
            review_page_url(&gh, "draft/x-01").unwrap(),
            "https://github.com/o/r/pull/new/draft/x-01"
        );
        // GitHub Enterprise uses the SELF-HOSTED host, not github.com.
        assert!(
            review_page_url(&ghe, "draft/x-01")
                .unwrap()
                .starts_with("https://github.acme.internal/o/r/pull/new/")
        );
        // GitLab is a merge request, with GitLab's URL scheme.
        assert!(
            review_page_url(&gl, "draft/x-01")
                .unwrap()
                .contains("/-/merge_requests/new?")
        );
        // An unrecognised host: no URL invented — the operator opens the request.
        assert_eq!(review_page_url(&other, "draft/x-01"), None);

        assert_eq!(host_kind("github.com").review_term(), "pull request");
        assert_eq!(host_kind("gitlab.com").review_term(), "merge request");
        assert_eq!(host_kind("bitbucket.org").review_term(), "review request");
    }

    #[test]
    fn review_create_args_are_per_cli() {
        let gh = review_create_args(HostKind::GitHub, "draft/x-01", "main", "Lower x").unwrap();
        assert_eq!(&gh[..2], ["pr", "create"]);
        assert_eq!(
            gh[gh.iter().position(|a| a == "--head").unwrap() + 1],
            "draft/x-01"
        );

        let gl = review_create_args(HostKind::GitLab, "draft/x-01", "main", "Lower x").unwrap();
        assert_eq!(&gl[..2], ["mr", "create"]);
        assert_eq!(
            gl[gl.iter().position(|a| a == "--source-branch").unwrap() + 1],
            "draft/x-01"
        );

        // No supported CLI for an unknown host.
        assert_eq!(review_create_args(HostKind::Other, "d", "main", "t"), None);
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
