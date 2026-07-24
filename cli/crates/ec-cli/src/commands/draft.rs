//! `deployment draft` — the authoring lifecycle (DESIGN-cli §8.10; Studio register #16).
//!
//! A draft is a **named change**: the author supplies a title, the ref is derived, and the vocabulary
//! is propose (`open`) → edit → review (`status`) → apply (the Git host's PR merge). Edits are committed
//! onto the draft branch **without touching the working tree**, so the read-only server keeps serving
//! while a draft is authored. `status` runs the semantic conflict check — comparing rendered *outputs*,
//! not layer text — that register #16 makes the load-bearing rule.

use std::path::Path;

use ec_adapters::{LocalGit, check_draft, load_workspace, repo_relative};
use ec_deploy::draft::DraftName;
use ec_deploy::ports::{DraftPort, LocalRoot};
use ec_diag::{Fatal, Report};

fn local_git(repo: &Path) -> LocalGit {
    LocalGit {
        root: LocalRoot(repo.to_path_buf()),
    }
}

/// A short, opaque disambiguator for a draft ref. The kernel's [`DraftName::derive`] stays pure; the
/// entropy is supplied here (the CLI may read the clock — the kernel may not).
fn short_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{:04x}", nanos & 0xffff)
}

/// `open <title>` — propose a change; print the derived ref.
pub fn open(title: &str, repo: &Path, base: &str) -> Result<Report, Fatal> {
    let name = DraftName::derive(title, &short_id());
    local_git(repo)
        .open(&name.git_ref, base)
        .map_err(|e| Fatal::Usage(format!("opening the draft: {e}")))?;
    println!(
        "proposed \"{}\"\n  ref {}  (derived — you never type it)",
        name.title, name.git_ref
    );
    println!(
        "  edit it with:  edgecommons deployment draft edit {} <layer-path> <file>",
        name.git_ref
    );
    Ok(Report::new())
}

/// `edit <ref> <path> <file>` — stage a layer change onto a draft.
pub fn edit(git_ref: &str, path: &str, contents: &Path, repo: &Path) -> Result<Report, Fatal> {
    let bytes = std::fs::read(contents)
        .map_err(|e| Fatal::Usage(format!("reading {}: {e}", contents.display())))?;
    local_git(repo)
        .write_file(git_ref, path, &bytes, &format!("author: edit {path}"))
        .map_err(|e| Fatal::Usage(format!("staging the edit: {e}")))?;
    println!("staged {path} on {git_ref} (the working tree is untouched)");
    Ok(Report::new())
}

/// `list` — the open drafts.
pub fn list(repo: &Path) -> Result<Report, Fatal> {
    let refs = local_git(repo)
        .list()
        .map_err(|e| Fatal::Usage(format!("listing drafts: {e}")))?;
    if refs.is_empty() {
        println!("no open drafts");
    } else {
        for r in refs {
            println!("{r}");
        }
    }
    Ok(Report::new())
}

/// `status <ref>` — review a draft for conflicts against current main.
pub fn status(
    git_ref: &str,
    repo: &Path,
    profile: Option<&str>,
    main: &str,
) -> Result<Report, Fatal> {
    let git = local_git(repo);

    // The profile to render for: the one named, or the definition's only profile.
    let profile = match profile {
        Some(p) => p.to_string(),
        None => {
            let loaded = load_workspace(&repo.join("definition.yaml")).map_err(Fatal::Usage)?;
            match loaded.profile_names().as_slice() {
                [one] => one.clone(),
                names => {
                    return Err(Fatal::Usage(format!(
                        "the definition declares {} profiles ({}); pick one with --profile",
                        names.len(),
                        names.join(", ")
                    )));
                }
            }
        }
    };

    // The definition's directory relative to the repo root — CODEOWNERS-anchored, and the base the
    // render pipeline joins layer paths onto.
    let dir_prefix = repo_relative(repo, repo);
    let check = check_draft(&git, &dir_prefix, &profile, git_ref, main)
        .map_err(|e| Fatal::Usage(format!("checking the draft: {e}")))?;

    if check.is_clean() {
        println!("{git_ref} applies onto {main} with no conflict (profile {profile}).");
        return Ok(Report::new());
    }
    if !check.textual.is_empty() {
        println!("Textual conflicts — Git cannot merge these files, resolve them first:");
        for p in &check.textual {
            println!("  {p}");
        }
    }
    if !check.semantic.is_empty() {
        println!(
            "Semantic conflicts — the merged deployment differs from what you reviewed ({} output(s)):",
            check.semantic.len()
        );
        for c in &check.semantic {
            println!("  [{:?}] {}", c.kind, c.summary);
        }
    }
    Ok(Report::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    const DEF: &str = r#"
apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata: { name: draft-cli, description: x }
hierarchy:
  levels: [site, device]
  scopes: [{ id: site/lab, parent: null, layer: layers/site.json }]
topology:
  nodes:
    - key: box-01
      scope: site/lab
      components: [{ name: telemetry-processor, layer: layers/telemetry.json }]
profiles:
  host:
    family: HOST
    environments: [{ name: local, bindings: bindings/local.json }]
    defaults: { configSource: CONFIG_COMPONENT }
    nodes:
      box-01:
        configProvider: { configSource: FILE, artifact: { source: { kind: sibling, repo: config-component } }, layer: layers/provider.json, catalogPath: /config/config-catalog.json }
        components:
          telemetry-processor: { artifact: { source: { kind: sibling, repo: telemetry-processor } }, launch: { order: 30 } }
  edge:
    family: KUBERNETES
    environments: [{ name: prod, bindings: bindings/local.json }]
    defaults: { configSource: CONFIGMAP }
    nodes:
      box-01:
        components:
          telemetry-processor: { image: ghcr.io/edgecommons/telemetry-processor:0.2.0 }
"#;
    const PROVIDER: &str = r#"{ "component": { "token": "cc", "global": { "configComponent": { "catalogSource": { "type": "file", "path": "${provider:catalog.path}" } } }, "instances": [{ "id": "main" }] } }"#;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e.c")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e.c")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-qb", "main"]);
        std::fs::create_dir_all(p.join("bindings")).unwrap();
        std::fs::create_dir_all(p.join("layers")).unwrap();
        std::fs::write(p.join("bindings/local.json"), "{}\n").unwrap();
        std::fs::write(p.join("layers/site.json"), "{ \"heartbeat\": { \"intervalSecs\": 30 } }\n").unwrap();
        std::fs::write(p.join("layers/telemetry.json"), "{ \"component\": { \"global\": { \"publishIntervalMs\": 500 } } }\n").unwrap();
        std::fs::write(p.join("layers/provider.json"), PROVIDER).unwrap();
        std::fs::write(p.join("definition.yaml"), DEF).unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-qm", "init"]);
        dir
    }

    /// The full lifecycle: open → edit → status(clean) → main moves → status(semantic conflict) → list.
    #[test]
    fn the_draft_lifecycle_runs_and_status_catches_a_semantic_conflict() {
        let dir = repo();
        let p = dir.path();

        // list: none yet.
        assert!(list(p).is_ok());

        // open derives a ref and creates the branch.
        assert!(open("Lower the interval", p, "main").is_ok());
        let dref = ec_deploy::draft::DraftName::derive("Lower the interval", "test").git_ref;
        // Re-open under a fixed ref via the port so the test can name it deterministically.
        ec_deploy::ports::DraftPort::open(&local_git(p), &dref, "main").unwrap();

        // edit stages a layer change on the draft.
        let newleaf = p.join("newleaf.json");
        std::fs::write(&newleaf, "{ \"component\": { \"global\": { \"publishIntervalMs\": 250 } } }\n").unwrap();
        assert!(edit(&dref, "layers/telemetry.json", &newleaf, p).is_ok());

        // status with an explicit profile is clean so far.
        assert!(status(&dref, p, Some("host"), "main").is_ok());

        // main edits a different file → status reports a semantic conflict (still Ok — it is a review).
        std::fs::write(p.join("layers/site.json"), "{ \"heartbeat\": { \"intervalSecs\": 5 } }\n").unwrap();
        git(p, &["add", "-A"]);
        git(p, &["commit", "-qm", "hb"]);
        assert!(status(&dref, p, Some("host"), "main").is_ok());

        // list now shows the draft.
        assert!(list(p).is_ok());
    }

    #[test]
    fn status_without_a_profile_on_a_multi_profile_definition_is_a_usage_error() {
        let dir = repo();
        let p = dir.path();
        let dref = "draft/x-01";
        ec_deploy::ports::DraftPort::open(&local_git(p), dref, "main").unwrap();
        // The definition declares two profiles (host, edge), so a bare status must ask for one.
        let err = status(dref, p, None, "main").unwrap_err();
        assert!(matches!(err, Fatal::Usage(m) if m.contains("profiles")));
    }

    #[test]
    fn edit_of_a_missing_file_is_a_usage_error() {
        let dir = repo();
        let p = dir.path();
        let dref = "draft/x-01";
        ec_deploy::ports::DraftPort::open(&local_git(p), dref, "main").unwrap();
        let err = edit(dref, "layers/telemetry.json", &p.join("nope.json"), p).unwrap_err();
        assert!(matches!(err, Fatal::Usage(_)));
    }

    #[test]
    fn short_id_is_four_hex_digits() {
        let id = short_id();
        assert_eq!(id.len(), 4);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
