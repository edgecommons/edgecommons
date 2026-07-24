//! End-to-end proof of the draft engine (register #16): open a draft, edit a layer without touching
//! the working tree, and detect a **semantic** conflict that a purely textual merge waves through.

use std::path::Path;
use std::process::Command;

use ec_adapters::{LocalGit, check_draft};
use ec_deploy::ports::LocalRoot;
use ec_deploy::ports::{DraftPort, GitPort};

// The real HOST shape: a CONFIG_COMPONENT provider, so each component's effective config is rendered
// into `config-catalog.json` / `config-component-config.json`. That is what makes a config edit visible
// in the rendered output the semantic detector compares — a pure `FILE` component without a provider
// renders no effective-config file at all.
const DEF: &str = r#"
apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata: { name: drafts-demo, description: a tiny plant }
hierarchy:
  levels: [site, device]
  scopes:
    - { id: site/lab, parent: null, layer: layers/site.json }
topology:
  nodes:
    - key: box-01
      scope: site/lab
      components:
        - name: telemetry-processor
          layer: layers/telemetry.json
profiles:
  host:
    family: HOST
    environments: [{ name: local, bindings: bindings/local.json }]
    defaults: { configSource: CONFIG_COMPONENT }
    nodes:
      box-01:
        configProvider:
          configSource: FILE
          artifact: { source: { kind: sibling, repo: config-component } }
          layer: layers/provider.json
          catalogPath: /config/config-catalog.json
        components:
          telemetry-processor:
            artifact: { source: { kind: sibling, repo: telemetry-processor } }
            launch: { order: 30 }
"#;

const LEAF: &str = "{ \"component\": { \"global\": { \"publishIntervalMs\": 500 } } }\n";
const SCOPE: &str = "{ \"heartbeat\": { \"intervalSecs\": 30 } }\n";
const PROVIDER: &str = r#"{
  "tags": { "componentRole": "configuration" },
  "component": {
    "token": "edgecommons-config-component",
    "global": {
      "configComponent": {
        "catalogSource": { "type": "file", "path": "${provider:catalog.path}", "watch": true }
      }
    },
    "instances": [{ "id": "main" }]
  }
}
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e.c")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e.c")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` whose root is the site, with the definition committed.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-qb", "main"]);
    std::fs::create_dir_all(p.join("bindings")).unwrap();
    std::fs::write(p.join("bindings/local.json"), "{}\n").unwrap();
    std::fs::create_dir_all(p.join("layers")).unwrap();
    std::fs::write(p.join("layers/telemetry.json"), LEAF).unwrap();
    std::fs::write(p.join("layers/site.json"), SCOPE).unwrap();
    std::fs::write(p.join("layers/provider.json"), PROVIDER).unwrap();
    std::fs::write(p.join("definition.yaml"), DEF).unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-qm", "initial"]);
    dir
}

fn local_git(dir: &Path) -> LocalGit {
    LocalGit {
        root: LocalRoot(dir.to_path_buf()),
    }
}

/// Commit a change on `main`, staging without disturbing anything the draft engine reads.
fn commit_on_main(p: &Path, rel: &str, contents: &str, msg: &str) {
    std::fs::write(p.join(rel), contents).unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-qm", msg]);
}

#[test]
fn a_draft_edit_is_committed_without_disturbing_the_working_tree() {
    let dir = repo();
    let p = dir.path();
    let g = local_git(p);

    let dref = "draft/raise-interval-01";
    g.open(dref, "main").unwrap();
    let before = std::fs::read_to_string(p.join("layers/telemetry.json")).unwrap();

    let edited = "{ \"component\": { \"global\": { \"publishIntervalMs\": 250 } } }\n";
    g.write_file(
        dref,
        "layers/telemetry.json",
        edited.as_bytes(),
        "raise the rate",
    )
    .unwrap();

    // Working tree untouched…
    assert_eq!(
        std::fs::read_to_string(p.join("layers/telemetry.json")).unwrap(),
        before
    );
    // …but the draft ref carries the edit, and the draft is listed.
    let on_draft = g
        .read_at(dref, "layers/telemetry.json")
        .unwrap()
        .expect("layer present on draft");
    assert!(String::from_utf8_lossy(&on_draft).contains("250"));
    assert_eq!(g.list().unwrap(), vec![dref.to_string()]);
}

#[test]
fn a_draft_that_lands_cleanly_reports_no_conflict() {
    let dir = repo();
    let p = dir.path();
    let g = local_git(p);

    let dref = "draft/raise-interval-01";
    g.open(dref, "main").unwrap();
    let edited = "{ \"component\": { \"global\": { \"publishIntervalMs\": 250 } } }\n";
    g.write_file(
        dref,
        "layers/telemetry.json",
        edited.as_bytes(),
        "raise the rate",
    )
    .unwrap();

    // Main moves only in a file the render never reads — the draft still applies clean.
    commit_on_main(p, "README.md", "hi\n", "docs");

    let check = check_draft(&g, "", "host", dref, "main").unwrap();
    assert!(check.is_clean(), "unexpected conflicts: {check:?}");
}

#[test]
fn a_textually_clean_merge_that_changes_the_effective_config_is_a_semantic_conflict() {
    // #16's canonical case: the draft and main touch *different files* that both feed one node, so Git
    // merges them without a textual conflict — but the effective config the author reviewed is not
    // what will deploy.
    let dir = repo();
    let p = dir.path();
    let g = local_git(p);

    // The draft lowers the component's interval and reviews that render.
    let dref = "draft/raise-interval-01";
    g.open(dref, "main").unwrap();
    g.write_file(
        dref,
        "layers/telemetry.json",
        "{ \"component\": { \"global\": { \"publishIntervalMs\": 250 } } }\n".as_bytes(),
        "raise the rate",
    )
    .unwrap();

    // Meanwhile main edits the *scope* layer — a different file — changing the heartbeat that also
    // merges into box-01's effective config.
    commit_on_main(
        p,
        "layers/site.json",
        "{ \"heartbeat\": { \"intervalSecs\": 5 } }\n",
        "faster heartbeat",
    );

    let check = check_draft(&g, "", "host", dref, "main").unwrap();
    assert!(
        check.textual.is_empty(),
        "the merge is textually clean (different files): {check:?}"
    );
    assert!(
        !check.semantic.is_empty(),
        "the semantic pass must catch the changed effective config: {check:?}"
    );
    assert!(
        check
            .semantic
            .iter()
            .any(|c| c.kind == ec_deploy::draft::ConflictKind::Altered),
        "expected an Altered conflict, got {:?}",
        check.semantic
    );
}

#[test]
fn a_same_line_edit_on_both_sides_is_a_textual_conflict() {
    let dir = repo();
    let p = dir.path();
    let g = local_git(p);

    let dref = "draft/raise-interval-01";
    g.open(dref, "main").unwrap();
    g.write_file(
        dref,
        "layers/telemetry.json",
        "{ \"component\": { \"global\": { \"publishIntervalMs\": 250 } } }\n".as_bytes(),
        "draft rate",
    )
    .unwrap();

    // Main rewrites the same single line to a different value → Git cannot merge it as text.
    commit_on_main(
        p,
        "layers/telemetry.json",
        "{ \"component\": { \"global\": { \"publishIntervalMs\": 999 } } }\n",
        "main rate",
    );

    let check = check_draft(&g, "", "host", dref, "main").unwrap();
    assert_eq!(check.textual, vec!["layers/telemetry.json".to_string()]);
}
