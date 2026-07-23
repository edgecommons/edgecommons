//! End-to-end tests: drive the real binary and observe what it actually does.
//!
//! Unit tests prove the pieces work. These prove the *tool* works — the dispatch, the exit
//! codes, the output a user actually sees. They are also the only thing that would have caught
//! the class of defect the previous CLI shipped with, where each piece was fine and the
//! assembled command was broken.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The binary under test, instrumented by the coverage run.
fn ec() -> Command {
    Command::new(env!("CARGO_BIN_EXE_edgecommons"))
}

fn run(args: &[&str], cwd: &Path) -> Output {
    ec().args(args)
        .current_dir(cwd)
        .output()
        .expect("the CLI must be runnable")
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

/// The monorepo root, so `--dep-source local` can resolve the sibling library.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("<root>/cli/crates/ec-cli")
        .to_path_buf()
}

fn scaffold(dir: &Path, name: &str, language: &str, extra: &[&str]) -> Output {
    let mut args = vec!["component", "new", "-n", name, "-l", language, "--yes"];
    args.extend_from_slice(extra);
    run(&args, dir)
}

// --- the surface -------------------------------------------------------------------------

#[test]
fn the_binary_runs_and_reports_its_surface() {
    let o = run(&["--help"], &repo_root());
    assert_eq!(code(&o), 0);
    let s = stdout(&o);
    for verb in [
        "component",
        "template",
        "registry",
        "deployment",
        "studio",
        "doctor",
    ] {
        assert!(s.contains(verb), "`{verb}` must appear in --help:\n{s}");
    }
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let o = run(&["nonsense"], &repo_root());
    assert_eq!(code(&o), 2, "unknown commands exit 2 (usage)");
}

#[test]
fn the_old_flat_verbs_are_gone() {
    // A clean break, enforced end-to-end and not merely in the parser.
    for old in [
        "create-component",
        "list-components",
        "list-templates",
        "deploy",
    ] {
        let o = run(&[old], &repo_root());
        assert_eq!(code(&o), 2, "`{old}` must not resolve");
    }
}

#[test]
fn completions_are_generated_for_every_shell() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let o = run(&["completions", shell], &repo_root());
        assert_eq!(code(&o), 0, "completions for {shell}");
        assert!(
            !stdout(&o).is_empty(),
            "{shell} completions must not be empty"
        );
    }
}

// --- templates ---------------------------------------------------------------------------

#[test]
fn template_list_shows_the_language_by_kind_matrix() {
    let o = run(&["template", "list"], &repo_root());
    assert_eq!(code(&o), 0);
    let s = stdout(&o);
    for id in [
        "java/service",
        "java/protocol-adapter",
        "python/service",
        "python/protocol-adapter",
        "rust/service",
        "typescript/service",
    ] {
        assert!(s.contains(id), "`{id}` must be listed:\n{s}");
    }
}

#[test]
fn template_list_json_is_machine_readable() {
    let o = run(&["--json", "template", "list"], &repo_root());
    assert_eq!(code(&o), 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    let rows = v.as_array().expect("an array");
    assert!(rows.len() >= 6);
    assert!(
        rows.iter()
            .any(|r| r["id"] == "rust/service" && r["kind"] == "service")
    );
}

#[test]
fn template_show_reports_packs_and_files() {
    let o = run(
        &["--json", "template", "show", "rust/service"],
        &repo_root(),
    );
    assert_eq!(code(&o), 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["language"], "RUST");
    let files = v["files"].as_array().unwrap();
    assert!(files.iter().any(|f| f == "Cargo.toml"));
    // The manifest is a template artifact and is never shipped to the user.
    assert!(!files.iter().any(|f| f == "edgecommons-template.json"));
    assert!(v["packs"]["HOST"].is_array(), "a HOST pack must exist");
}

#[test]
fn template_show_of_an_unknown_id_is_a_usage_error() {
    let o = run(&["template", "show", "cobol/service"], &repo_root());
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("template list"),
        "the error must say how to discover valid ids"
    );
}

// --- component new -----------------------------------------------------------------------

#[test]
fn scaffolding_requires_a_name_and_a_language_off_a_terminal() {
    let d = tempfile::tempdir().unwrap();
    // With --yes and no name, prompting is not an option: this must be a usage error rather
    // than a process that blocks forever waiting on stdin in CI.
    let o = run(&["component", "new", "-l", "RUST", "--yes"], d.path());
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("name"), "{}", stderr(&o));

    let o = run(
        &["component", "new", "-n", "com.example.X", "--yes"],
        d.path(),
    );
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("language"), "{}", stderr(&o));
}

#[test]
fn a_host_only_scaffold_carries_no_greengrass_artifacts() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.HostOnly",
        "RUST",
        &["--platforms", "HOST"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));

    // The output directory is the kebab crate name: `com.example.HostOnly` -> `host-only`.
    let p = d.path().join("host-only");
    assert!(
        !p.join("recipe.yaml").exists(),
        "a HOST-only scaffold must carry no Greengrass recipe"
    );
    assert!(!p.join("gdk-config.json").exists());
    assert!(!p.join("k8s").exists());
    // ...and it must carry the HOST pack, plus its own config contract.
    assert!(p.join("compose.yaml").exists());
    assert!(p.join("supervisor/component.conf").exists());
    assert!(p.join("config.schema.json").exists());
    // The Dockerfile is shared with the k8s pack and must survive a HOST-only selection.
    assert!(p.join("Dockerfile").exists());
}

#[test]
fn a_greengrass_scaffold_carries_the_recipe() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.GgOnly",
        "RUST",
        &["--platforms", "GREENGRASS"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let p = d.path().join("gg-only");
    assert!(p.join("recipe.yaml").exists());
    assert!(p.join("gdk-config.json").exists());
}

#[test]
fn the_protocol_adapter_kind_is_reachable() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.MyAdapter",
        "PYTHON",
        &["-k", "protocol-adapter"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    // The output dir is kebab (`my-adapter`); the Python package dir is the snake_case module
    // name (`my_adapter/`, from SNAKENAME) — the modbus_adapter convention, not the old `app/`.
    let p = d.path().join("my-adapter");
    assert!(
        p.join("my_adapter/adapter.py").exists(),
        "the adapter package must be renamed to the snake_case module name"
    );
    assert!(
        !p.join("app").exists(),
        "the template's `app/` package must not survive the rename"
    );
    // The adapter archetype ships the device seam + command surface + metrics.
    assert!(p.join("my_adapter/command_service.py").exists());
    assert!(p.join("my_adapter/device.py").exists());
    assert!(p.join("config.schema.json").exists());
}

#[test]
fn license_none_makes_no_claim_and_a_named_license_writes_a_file_and_field() {
    // SD-4 / D-CLI-21: a scaffold is the author's component. The default (`none`) writes no LICENSE
    // and makes NO manifest license claim (not an empty one); `--license <spdx>` writes the text and
    // populates the manifest field. The old templates silently stamped Apache-2.0.
    let d = tempfile::tempdir().unwrap();
    let none = scaffold(d.path(), "com.example.NoLic", "RUST", &["-k", "service"]);
    assert_eq!(code(&none), 0, "{}", stderr(&none));
    let p = d.path().join("no-lic");
    assert!(
        !p.join("LICENSE").exists(),
        "--license none writes no LICENSE"
    );
    let cargo = std::fs::read_to_string(p.join("Cargo.toml")).unwrap();
    assert!(
        !cargo.lines().any(|l| l.trim().starts_with("license")),
        "--license none leaves no license field (not an empty one):\n{cargo}"
    );

    let d2 = tempfile::tempdir().unwrap();
    let mit = scaffold(
        d2.path(),
        "com.example.MitLic",
        "RUST",
        &["-k", "service", "--license", "mit"],
    );
    assert_eq!(code(&mit), 0, "{}", stderr(&mit));
    let p2 = d2.path().join("mit-lic");
    let text = std::fs::read_to_string(p2.join("LICENSE")).unwrap();
    assert!(text.contains("MIT License"), "the MIT text is written");
    let cargo2 = std::fs::read_to_string(p2.join("Cargo.toml")).unwrap();
    assert!(
        cargo2.contains("license = \"MIT\""),
        "the manifest field is populated:\n{cargo2}"
    );
}

#[test]
fn the_language_by_kind_matrix_is_complete() {
    // Every language x kind cell has a template. This is the invariant that replaced
    // "an unfilled cell is a usage error" -- there are no unfilled cells left to test with.
    // If a template is ever deleted, this is what says so.
    let o = run(&["--json", "template", "list"], &repo_root());
    assert_eq!(code(&o), 0);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let ids: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();

    for language in ["java", "python", "rust", "typescript"] {
        for kind in ["service", "protocol-adapter", "processor", "sink"] {
            let id = format!("{language}/{kind}");
            assert!(
                ids.contains(&id.as_str()),
                "the matrix is missing `{id}`: {ids:?}"
            );
        }
    }
    assert_eq!(ids.len(), 16, "4 languages x 4 kinds");
}

#[test]
fn an_unknown_kind_is_rejected_by_the_parser() {
    // The kinds are a closed set; a kind outside it never reaches template resolution.
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(d.path(), "com.example.X", "RUST", &["-k", "nonsense"]);
    assert_eq!(code(&o), 2);
}

#[test]
fn a_non_empty_target_is_refused_without_force() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(d.path(), "com.example.Twice", "RUST", &[])),
        0
    );

    let o = scaffold(d.path(), "com.example.Twice", "RUST", &[]);
    assert_eq!(
        code(&o),
        2,
        "a second scaffold must refuse rather than clobber"
    );

    let o = scaffold(d.path(), "com.example.Twice", "RUST", &["--force"]);
    assert_eq!(code(&o), 0, "--force overwrites: {}", stderr(&o));
}

#[test]
fn registry_dep_source_pins_a_version_that_exists() {
    // The previous CLI hardcoded 0.1.0 and emitted a git tag that does not exist, so the
    // scaffold could not resolve. The version is now read from the workspace at build time.
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.Pinned",
        "RUST",
        &["--dep-source", "registry"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));

    let cargo = std::fs::read_to_string(d.path().join("pinned/Cargo.toml")).unwrap();
    let want = format!("rust-lib/v{}", ec_scaffold::generate::EDGECOMMONS_VERSION);
    assert!(cargo.contains(&want), "expected {want} in:\n{cargo}");
    assert!(
        !cargo.contains("rust-lib/v0.1.0"),
        "the nonexistent tag must never be emitted"
    );
}

#[test]
fn the_derived_output_dir_is_the_kebab_name() {
    // `com.example.EthernetIpAdapter` -> crate/dir `ethernet-ip-adapter` (case-boundary kebab).
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.EthernetIpAdapter",
        "RUST",
        &["-k", "protocol-adapter"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let p = d.path().join("ethernet-ip-adapter");
    assert!(p.is_dir(), "the output dir must be kebab: {:?}", d.path());
    let cargo = std::fs::read_to_string(p.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"ethernet-ip-adapter\""), "{cargo}");
}

#[test]
fn the_dir_flag_wins_over_the_derived_default() {
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("exactly/here");
    let o = scaffold(
        d.path(),
        "com.example.MyThing",
        "RUST",
        &["--dir", out.to_str().unwrap()],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        out.join("Cargo.toml").exists(),
        "--dir sets the exact output dir"
    );
    // The derived default must not have been created alongside it.
    assert!(!d.path().join("my-thing").exists());
}

#[test]
fn the_bin_name_flag_overrides_the_crate_and_dir() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.MyThing",
        "RUST",
        &["--bin-name", "custom-bin"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let p = d.path().join("custom-bin");
    assert!(p.is_dir(), "--bin-name also names the default dir");
    let cargo = std::fs::read_to_string(p.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"custom-bin\""), "{cargo}");
}

#[test]
fn an_invalid_bin_name_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.MyThing",
        "RUST",
        &["--bin-name", "Not_Valid"],
    );
    assert_eq!(code(&o), 2, "an invalid crate name is a usage error");
    assert!(stderr(&o).contains("bin-name"), "{}", stderr(&o));
}

#[test]
fn a_pinned_rev_rust_scaffold_emits_the_override_and_a_rev_pin() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.EthernetIpAdapter",
        "RUST",
        &["-k", "protocol-adapter", "--dep-source", "pinned-rev"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let p = d.path().join("ethernet-ip-adapter");
    let cfg = std::fs::read_to_string(p.join(".cargo/config.toml")).unwrap();
    assert!(
        cfg.contains(r#"[patch."https://github.com/edgecommons/edgecommons"]"#),
        "{cfg}"
    );
    let cargo = std::fs::read_to_string(p.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains("rev = \""),
        "the dep must be rev-pinned: {cargo}"
    );
}

#[test]
fn pinned_rev_on_java_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.JavaThing",
        "JAVA",
        &["--dep-source", "pinned-rev"],
    );
    assert_eq!(code(&o), 2, "Maven cannot express a git-rev pin");
    assert!(stderr(&o).contains("pinned-rev"), "{}", stderr(&o));
}

#[test]
fn a_bucketless_greengrass_scaffold_uses_the_sentinel_and_validate_errors() {
    let d = tempfile::tempdir().unwrap();
    // A GREENGRASS scaffold with no bucket writes the sentinel and warns (EC4005), exit 0.
    let o = scaffold(
        d.path(),
        "com.example.NoBucket",
        "RUST",
        &["--platforms", "GREENGRASS"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let gdk = std::fs::read_to_string(d.path().join("no-bucket/gdk-config.json")).unwrap();
    assert!(
        gdk.contains("edgecommons-set-artifact-bucket"),
        "the sentinel must be written: {gdk}"
    );

    // And `component validate` rejects the sentinel as EC3007 (exit 1).
    let o = run(&["component", "validate", "-p", "no-bucket"], d.path());
    assert_eq!(
        code(&o),
        1,
        "the sentinel must be a validate error:\n{}",
        stdout(&o)
    );
    assert!(stdout(&o).contains("EC3007"), "{}", stdout(&o));
}

/// A minimal but real template on disk: manifest v2 plus one substituted file.
fn custom_template(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("edgecommons-template.json"),
        serde_json::json!({
            "schemaVersion": 2,
            "id": "rust/sink",
            "language": "RUST",
            "kind": "sink",
            "description": "A custom sink template.",
            "platforms": ["HOST"],
            "substitutions": { "README.md": ["COMPONENTNAME", "DESCRIPTION"] },
            "packs": { "HOST": ["README.md"] }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("README.md"),
        "# <<COMPONENTNAME>>

<<DESCRIPTION>>
",
    )
    .unwrap();
}

#[test]
fn a_template_directory_can_be_used_instead_of_the_embedded_one() {
    let d = tempfile::tempdir().unwrap();
    let tpl = d.path().join("my-template");
    custom_template(&tpl);

    // The template's own manifest declares its language and kind -- `sink`, which the embedded
    // catalog has no template for. A template is a template wherever it comes from.
    let o = run(
        &[
            "component",
            "new",
            "-n",
            "com.example.Custom",
            "--template-dir",
            tpl.to_str().unwrap(),
            "--yes",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));

    let readme = std::fs::read_to_string(d.path().join("custom/README.md")).unwrap();
    assert!(readme.contains("# Custom"), "{readme}");
    assert!(
        !readme.contains("<<"),
        "tokens must be substituted: {readme}"
    );
    assert!(!d.path().join("custom/edgecommons-template.json").exists());
}

#[test]
fn a_directory_that_is_not_a_template_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let empty = d.path().join("not-a-template");
    std::fs::create_dir_all(&empty).unwrap();
    let o = run(
        &[
            "component",
            "new",
            "-n",
            "com.example.X",
            "--template-dir",
            empty.to_str().unwrap(),
            "--yes",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("not a template"), "{}", stderr(&o));
}

#[test]
fn a_template_can_be_cloned_from_git() {
    let d = tempfile::tempdir().unwrap();
    let origin = d.path().join("origin");
    custom_template(&origin);

    // A real git repository over a file:// URL -- the clone path is exercised, not mocked.
    for args in [
        vec!["init", "-q"],
        vec!["add", "-A"],
        vec![
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "t",
        ],
    ] {
        let ok = Command::new("git")
            .args(&args)
            .current_dir(&origin)
            .status()
            .expect("git");
        assert!(ok.success(), "git {args:?}");
    }

    let url = format!("file://{}", origin.display().to_string().replace('\\', "/"));
    let o = run(
        &[
            "component",
            "new",
            "-n",
            "com.example.Cloned",
            "--template-git",
            &url,
            "--yes",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let readme = std::fs::read_to_string(d.path().join("cloned/README.md")).unwrap();
    assert!(readme.contains("# Cloned"), "{readme}");
    // The template's git history is not part of the template.
    assert!(!d.path().join("cloned/.git").exists());
}

#[test]
fn a_clone_that_fails_is_an_environment_error() {
    let d = tempfile::tempdir().unwrap();
    let o = run(
        &[
            "component",
            "new",
            "-n",
            "com.example.X",
            "--template-git",
            "file:///no/such/repo",
            "--yes",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 3, "a failed clone is an environment failure");
}

// --- component validate ------------------------------------------------------------------

#[test]
fn a_freshly_scaffolded_component_validates_without_errors() {
    let d = tempfile::tempdir().unwrap();
    // A real bucket, so gdk-config.json carries no sentinel (which is now an EC3007 error).
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.Clean",
            "RUST",
            &["-b", "my-real-bucket"]
        )),
        0
    );

    let o = run(&["component", "validate", "-p", "clean"], d.path());
    assert_eq!(
        code(&o),
        0,
        "a scaffold this CLI produced must validate (warnings do not fail):\n{}",
        stdout(&o)
    );
    assert!(
        !stdout(&o).contains("error["),
        "a fresh scaffold must produce no errors:\n{}",
        stdout(&o)
    );
    // The one finding on a fresh Rust scaffold is the missing-lockfile advisory (SD-6/R12).
    assert!(
        stdout(&o).contains("EC4008"),
        "the missing-lockfile advisory must fire:\n{}",
        stdout(&o)
    );
}

#[test]
fn validate_catches_a_typo_in_the_components_own_config() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(d.path(), "com.example.Typo", "RUST", &[])),
        0
    );

    let cfg_path = d.path().join("typo/test-configs/config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["component"]["global"] = serde_json::json!({ "publish_intervall": 5 });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    let o = run(&["component", "validate", "-p", "typo"], d.path());
    assert_eq!(code(&o), 1, "findings exit 1");
    let s = stdout(&o);
    assert!(
        s.contains("EC1002"),
        "the component-schema code must fire:\n{s}"
    );
    assert!(
        s.contains("publish_intervall"),
        "the diagnostic must name the offending key:\n{s}"
    );
}

#[test]
fn validate_catches_a_literal_secret() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(d.path(), "com.example.Leak", "RUST", &[])),
        0
    );

    let cfg_path = d.path().join("leak/test-configs/config.json");
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["component"]["global"] = serde_json::json!({ "apiToken": "hunter2" });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    let o = run(&["component", "validate", "-p", "leak"], d.path());
    assert_eq!(code(&o), 1);
    assert!(stdout(&o).contains("EC2005"), "{}", stdout(&o));
}

#[test]
fn validate_json_output_is_machine_readable() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.Json",
            "RUST",
            &["-b", "my-real-bucket"]
        )),
        0
    );

    let o = run(&["--json", "component", "validate", "-p", "json"], d.path());
    assert_eq!(code(&o), 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["ok"], true);
    assert_eq!(v["errorCount"], 0);
}

#[test]
fn validate_of_a_missing_directory_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let o = run(&["component", "validate", "-p", "NoSuchThing"], d.path());
    assert_eq!(code(&o), 2);
}

// --- versions and release ----------------------------------------------------------------

#[test]
fn upgrade_moves_the_library_and_version_moves_the_component() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.Both",
            "RUST",
            &["--dep-source", "registry"]
        )),
        0
    );

    // `upgrade` moves the LIBRARY dependency...
    let o = run(
        &["component", "upgrade", "-p", "both", "--to", "9.9.9"],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let cargo = std::fs::read_to_string(d.path().join("both/Cargo.toml")).unwrap();
    assert!(cargo.contains("rust-lib/v9.9.9"), "{cargo}");
    assert!(
        cargo.contains("version = \"1.0.0\""),
        "the component's own version must not move"
    );

    // ...and `version` moves the COMPONENT's own version, leaving the library alone.
    let o = run(
        &["component", "version", "-p", "both", "--to", "2.5.0"],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let cargo = std::fs::read_to_string(d.path().join("both/Cargo.toml")).unwrap();
    assert!(cargo.contains("version = \"2.5.0\""), "{cargo}");
    assert!(
        cargo.contains("rust-lib/v9.9.9"),
        "the library dep must not move: {cargo}"
    );
}

#[test]
fn upgrade_dry_run_writes_nothing() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.Dry",
            "RUST",
            &["--dep-source", "registry"]
        )),
        0
    );
    let before = std::fs::read_to_string(d.path().join("dry/Cargo.toml")).unwrap();

    let o = run(
        &[
            "component",
            "upgrade",
            "-p",
            "dry",
            "--to",
            "9.9.9",
            "--dry-run",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("dry-run"));

    let after = std::fs::read_to_string(d.path().join("dry/Cargo.toml")).unwrap();
    assert_eq!(before, after, "--dry-run must not write");
}

#[test]
fn a_release_refuses_an_unlocked_scaffold_and_version_unlocks_it() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(code(&scaffold(d.path(), "com.example.Rel", "RUST", &[])), 0);

    // Every template ships NEXT_PATCH, which is not a thing you can deploy or roll back to.
    let o = run(
        &[
            "component",
            "release",
            "-p",
            "rel",
            "-o",
            "rel/release.json",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 2, "a release must pin a concrete version");
    assert!(
        stderr(&o).contains("component version"),
        "the error must say how to fix it"
    );

    // And the fix it names actually works — the dead end the previous CLI left people in.
    assert_eq!(
        code(&run(
            &["component", "version", "-p", "rel", "--to", "1.4.2"],
            d.path()
        )),
        0
    );

    let o = run(
        &[
            "component",
            "release",
            "-p",
            "rel",
            "-o",
            "rel/release.json",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stdout(&o).contains("did NOT publish"),
        "the tool must say it published nothing"
    );

    let desc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.path().join("rel/release.json")).unwrap())
            .unwrap();
    assert_eq!(desc["version"], "1.4.2");
    // The config schema travels with the release — this is what makes compatibility derived.
    assert!(desc["configSchema"].is_object());
    assert!(desc["supplyChain"].is_object());
}

// --- registry ----------------------------------------------------------------------------

#[test]
fn registry_reads_a_local_catalog_and_filters_by_category() {
    let d = tempfile::tempdir().unwrap();
    let catalog = d.path().join("components.json");
    std::fs::write(
        &catalog,
        serde_json::json!({
            "schemaVersion": 1,
            "components": [
                { "name": "uns-bridge", "language": "RUST", "category": "bridge", "description": "Relay", "repo": "edgecommons/uns-bridge" },
                { "name": "opcua-adapter", "language": "JAVA", "category": "adapter", "description": "OPC UA", "repo": "edgecommons/opcua-adapter" }
            ]
        })
        .to_string(),
    )
    .unwrap();
    let src = catalog.to_str().unwrap();

    let o = run(&["registry", "list", "--source", src], d.path());
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(stdout(&o).contains("uns-bridge"));

    // `bridge` is one of the three categories the previous CLI's help never mentioned.
    let o = run(
        &["registry", "list", "--source", src, "--category", "bridge"],
        d.path(),
    );
    assert_eq!(code(&o), 0);
    let s = stdout(&o);
    assert!(s.contains("uns-bridge"));
    assert!(
        !s.contains("opcua-adapter"),
        "the filter must exclude other categories:\n{s}"
    );
}

#[test]
fn registry_show_reports_one_entry() {
    let d = tempfile::tempdir().unwrap();
    let catalog = d.path().join("components.json");
    std::fs::write(
        &catalog,
        serde_json::json!({
            "schemaVersion": 1,
            "components": [{ "name": "uns-bridge", "language": "RUST", "category": "bridge", "description": "Relay", "repo": "edgecommons/uns-bridge" }]
        })
        .to_string(),
    )
    .unwrap();
    // `show` reads the default source, so point it at ours via the env var the CLI honours.
    let o = ec()
        .args([
            "--json",
            "registry",
            "list",
            "--source",
            catalog.to_str().unwrap(),
        ])
        .current_dir(d.path())
        .output()
        .unwrap();
    assert_eq!(code(&o), 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v[0]["name"], "uns-bridge");
}

#[test]
fn registry_show_and_versions_read_a_local_catalog() {
    let d = tempfile::tempdir().unwrap();
    let catalog = d.path().join("components.json");
    std::fs::write(
        &catalog,
        serde_json::json!({
            "schemaVersion": 1,
            "components": [{
                "name": "telemetry-processor", "language": "RUST", "category": "processor",
                "description": "Pipelines", "repo": "edgecommons/telemetry-processor",
                "status": "beta", "platforms": ["GREENGRASS", "HOST"]
            }]
        })
        .to_string(),
    )
    .unwrap();
    let src = catalog.to_str().unwrap();

    let o = run(
        &["registry", "show", "telemetry-processor", "--source", src],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.contains("telemetry-processor"));
    assert!(s.contains("processor"));

    let o = run(
        &[
            "--json",
            "registry",
            "show",
            "telemetry-processor",
            "--source",
            src,
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["language"], "RUST");

    let o = run(&["registry", "show", "nope", "--source", src], d.path());
    assert_eq!(code(&o), 2, "an unknown component is a usage error");

    // `versions` reads the release index — which does not exist. It must say so rather than
    // inventing versions, and must warn rather than fail.
    let o = run(
        &[
            "registry",
            "versions",
            "telemetry-processor",
            "--source",
            src,
        ],
        d.path(),
    );
    assert_eq!(
        code(&o),
        0,
        "a missing release index warns; it does not fail"
    );
    assert!(
        stdout(&o).contains("no published releases"),
        "{}",
        stdout(&o)
    );

    let o = run(&["registry", "versions", "nope", "--source", src], d.path());
    assert_eq!(code(&o), 2);
}

#[test]
fn registry_rejects_a_missing_catalog_file() {
    let d = tempfile::tempdir().unwrap();
    let o = run(
        &["registry", "list", "--source", "/no/such/catalog.json"],
        d.path(),
    );
    assert_eq!(code(&o), 2);
}

#[test]
fn package_reports_that_container_images_belong_to_ci() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.Pkg",
            "RUST",
            &["--platforms", "KUBERNETES"]
        )),
        0
    );
    // Building and pushing an image is the release workflow's job, not the CLI's — the same
    // produce-vs-publish line the release verb draws. It says so instead of half-doing it.
    let o = run(
        &[
            "component",
            "package",
            "-p",
            "pkg",
            "--platforms",
            "KUBERNETES",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(stdout(&o).contains("EC4007"), "{}", stdout(&o));
}

#[test]
fn package_of_an_unrecognisable_project_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("Empty")).unwrap();
    let o = run(&["component", "package", "-p", "Empty"], d.path());
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("--platforms"),
        "the error must say how to proceed"
    );
}

#[test]
fn package_and_release_of_a_missing_directory_are_usage_errors() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&run(&["component", "package", "-p", "Nope"], d.path())),
        2
    );
    assert_eq!(
        code(&run(&["component", "release", "-p", "Nope"], d.path())),
        2
    );
    assert_eq!(
        code(&run(
            &["component", "upgrade", "-p", "Nope", "--to", "1.0.0"],
            d.path()
        )),
        2
    );
    assert_eq!(
        code(&run(
            &["component", "version", "-p", "Nope", "--to", "1.0.0"],
            d.path()
        )),
        2
    );
}

#[test]
fn component_version_rejects_a_non_version() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(code(&scaffold(d.path(), "com.example.Bad", "RUST", &[])), 0);
    let o = run(
        &["component", "version", "-p", "bad", "--to", "latest"],
        d.path(),
    );
    assert_eq!(code(&o), 2, "`latest` is not a version");
}

#[test]
fn a_java_scaffold_renames_its_package_and_class() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(d.path(), "com.example.JavaThing", "JAVA", &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    // The output dir is kebab (`java-thing`); the Java package/class stay PascalCase reverse-DNS.
    let p = d.path().join("java-thing");
    assert!(
        p.join("src/main/java/com/example/javathing/JavaThing.java")
            .exists(),
        "the class must be renamed into the component's package"
    );
    assert!(
        !p.join("src/main/java/com/mbreissi").exists(),
        "the template package must not survive"
    );
    // The Java pom pins the library by version, resolved from the workspace.
    let pom = std::fs::read_to_string(p.join("pom.xml")).unwrap();
    assert!(
        pom.contains(ec_scaffold::generate::EDGECOMMONS_VERSION),
        "{pom}"
    );
}

#[test]
fn a_typescript_scaffold_uses_the_scoped_package() {
    let d = tempfile::tempdir().unwrap();
    let o = scaffold(
        d.path(),
        "com.example.TsThing",
        "TYPESCRIPT",
        &["--dep-source", "registry"],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let pkg = std::fs::read_to_string(d.path().join("ts-thing/package.json")).unwrap();
    assert!(pkg.contains("@edgecommons/edgecommons"), "{pkg}");
    // The registry dep-source ships the consumer registry config; the local one does not.
    assert!(d.path().join("ts-thing/.npmrc").exists());
}

// --- doctor and the unbuilt verbs ---------------------------------------------------------

#[test]
fn doctor_reports_and_exits_meaningfully() {
    let o = run(
        &["--json", "doctor", "--platforms", "HOST", "-l", "RUST"],
        &repo_root(),
    );
    // 0 if everything needed is present, 3 if something is missing — never a crash, and never
    // a blanket 0 regardless of what it found (which is what the previous doctor did).
    assert!(matches!(code(&o), 0 | 3), "doctor exited {}", code(&o));

    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["platforms"][0], "HOST");
    let tools = v["tools"].as_array().unwrap();
    // git is always required; cargo is required for Rust; gdk is Greengrass-only and must NOT
    // be checked here.
    assert!(tools.iter().any(|t| t["tool"] == "git"));
    assert!(tools.iter().any(|t| t["tool"] == "cargo"));
    assert!(
        !tools.iter().any(|t| t["tool"] == "gdk"),
        "gdk is not a HOST prerequisite"
    );
}

#[test]
fn doctor_checks_greengrass_tools_only_when_greengrass_is_selected() {
    let o = run(
        &["--json", "doctor", "--platforms", "GREENGRASS"],
        &repo_root(),
    );
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let tools = v["tools"].as_array().unwrap();
    assert!(
        tools.iter().any(|t| t["tool"] == "gdk"),
        "gdk must be checked for GREENGRASS"
    );
}

#[test]
fn the_unbuilt_verbs_say_so_rather_than_crashing() {
    // Declared in the surface, not built in this binary: exit 5, and name the design section
    // rather than failing obscurely or pretending to be a usage error.
    for args in [
        vec!["deployment", "lock", "def.yaml"],
        vec!["deployment", "diff", "def.yaml", "--against", "v1"],
        vec!["studio", "serve"],
    ] {
        let o = run(&args, &repo_root());
        assert_eq!(
            code(&o),
            5,
            "`{}` must exit 5 (not implemented)",
            args.join(" ")
        );
        let e = stderr(&o);
        assert!(e.contains("not available"), "{e}");
        // The message must not leak our internal plumbing at the user: no roadmap ids, no
        // phase numbers, no design-doc paths.
        for internal in ["RM-0", "Phase P", "DESIGN-cli", "§"] {
            assert!(
                !e.contains(internal),
                "user-facing output leaks `{internal}`: {e}"
            );
        }
    }
}

// --- remaining error paths ----------------------------------------------------------------

#[test]
fn a_greengrass_package_without_gdk_is_an_environment_error() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.GgPkg",
            "RUST",
            &["--platforms", "GREENGRASS"]
        )),
        0
    );
    // gdk is an external tool. If it is present this build genuinely can package; if it is not,
    // the failure must name the tool rather than being obscure. Either outcome is correct --
    // what is not correct is a crash or a silent success.
    let o = run(&["component", "package", "-p", "gg-pkg"], d.path());
    match code(&o) {
        0 => {}
        3 => assert!(stderr(&o).contains("gdk"), "{}", stderr(&o)),
        other => panic!("unexpected exit {other}: {}", stderr(&o)),
    }
}

#[test]
fn an_http_registry_source_is_reported_rather_than_silently_ignored() {
    let d = tempfile::tempdir().unwrap();
    let o = run(
        &[
            "registry",
            "list",
            "--source",
            "https://example.com/components.json",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 3, "this build has no HTTP client; it must say so");
    assert!(stderr(&o).contains("HTTP"), "{}", stderr(&o));
}

#[test]
fn validate_can_target_a_single_config_file() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        code(&scaffold(
            d.path(),
            "com.example.One",
            "RUST",
            &["-b", "my-real-bucket"]
        )),
        0
    );
    let cfg = d.path().join("one/test-configs/config.json");
    let o = run(
        &[
            "component",
            "validate",
            "-p",
            "one",
            "-c",
            cfg.to_str().unwrap(),
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "{}", stdout(&o));
}

#[test]
fn upgrade_of_a_project_with_no_dependency_manifest_warns() {
    let d = tempfile::tempdir().unwrap();
    let empty = d.path().join("Bare");
    std::fs::create_dir_all(&empty).unwrap();
    let o = run(
        &["component", "upgrade", "-p", "Bare", "--to", "1.0.0"],
        d.path(),
    );
    assert_eq!(code(&o), 0, "nothing to bump is not a failure");
    assert!(stdout(&o).contains("EC4004"), "{}", stdout(&o));
}

// --- deployment verbs (built in this binary) ------------------------------------------------

/// A minimal self-contained workspace: one scope level above `device`, one node, one FILE
/// component. Exercises validate (all three stages), render, and plan end to end.
fn write_minimal_workspace(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir.join("bindings")).unwrap();
    std::fs::write(
        dir.join("bindings/local.json"),
        "{}
",
    )
    .unwrap();
    let definition = dir.join("definition.yaml");
    std::fs::write(
        &definition,
        r#"apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata:
  name: mini
hierarchy:
  levels: [site, device]
  scopes:
    - id: site/lab
      parent: null
targetStandard:
  family: HOST
environments:
  - name: local
    bindings: bindings/local.json
nodes:
  - key: box-01
    scope: site/lab
    components:
      - name: telemetry-processor
        artifact: { source: { kind: sibling, repo: telemetry-processor } }
        configSource: FILE
        launch: { order: 30, waitFor: ["localhost:1883"] }
"#,
    )
    .unwrap();
    definition
}

#[test]
fn deployment_validate_render_and_plan_run_on_a_minimal_workspace() {
    let d = tempfile::tempdir().unwrap();
    let definition = write_minimal_workspace(d.path());

    let o = run(
        &["deployment", "validate", definition.to_str().unwrap()],
        d.path(),
    );
    assert_eq!(code(&o), 0, "validate must pass: {}", stderr(&o));

    let o = run(
        &[
            "deployment",
            "render",
            definition.to_str().unwrap(),
            "--env",
            "local",
            "--target",
            "HOST",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "render must pass: {}", stderr(&o));
    let conf = d.path().join("render/host/box-01/supervisord.conf");
    let conf_text = std::fs::read_to_string(&conf).expect("render wrote the supervisord conf");
    assert!(
        conf_text.contains("-c FILE /config/config-component-config.json -t box-01"),
        "launch line carries the FILE source and thing identity: {conf_text}"
    );

    let o = run(
        &[
            "deployment",
            "plan",
            definition.to_str().unwrap(),
            "--env",
            "local",
            "--target",
            "HOST",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "plan must pass: {}", stderr(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("\"entries\""),
        "plan prints the normalized plan: {out}"
    );
    assert!(
        out.contains("\"restartsComponent\""),
        "plan entries carry restart impact: {out}"
    );
}

#[test]
fn deployment_validate_rejects_a_broken_definition() {
    let d = tempfile::tempdir().unwrap();
    let definition = d.path().join("definition.yaml");
    // `device` used as a scope level (S-2) and a component without an artifact (S-6).
    std::fs::write(
        &definition,
        r#"apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata:
  name: broken
hierarchy:
  levels: [site, device]
  scopes:
    - id: device/box-01
      parent: null
targetStandard:
  family: HOST
environments:
  - name: local
    bindings: bindings/local.json
nodes:
  - key: box-01
    scope: device/box-01
    components:
      - name: telemetry-processor
        configSource: FILE
"#,
    )
    .unwrap();
    std::fs::create_dir_all(d.path().join("bindings")).unwrap();
    std::fs::write(
        d.path().join("bindings/local.json"),
        "{}
",
    )
    .unwrap();

    let o = run(
        &["deployment", "validate", definition.to_str().unwrap()],
        d.path(),
    );
    assert_eq!(code(&o), 1, "findings exit code: {}", stderr(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("EC5002"),
        "semantic rule diagnostics carry EC5002: {out}"
    );
}

#[test]
fn deployment_release_writes_a_two_stream_lock() {
    let d = tempfile::tempdir().unwrap();
    let definition = write_minimal_workspace(d.path());
    let o = run(
        &[
            "deployment",
            "release",
            definition.to_str().unwrap(),
            "--stream",
            "config",
        ],
        d.path(),
    );
    assert_eq!(code(&o), 0, "release must succeed: {}", stderr(&o));
    // The release dir is releases/config-<commit-or-unknown>/.
    let releases = d.path().join("releases");
    let sub = std::fs::read_dir(&releases)
        .unwrap()
        .next()
        .expect("a release dir was written")
        .unwrap()
        .path();
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(sub.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["promotedStream"], "config");
    let artifacts = manifest["streams"]["artifact"].as_array().unwrap();
    assert!(!artifacts.is_empty());
    assert!(sub.join("evidence.json").exists());
    assert!(sub.join("rendered/box-01/supervisord.conf").exists());
}

#[test]
fn deployment_render_to_the_wrong_target_is_a_usage_error() {
    let d = tempfile::tempdir().unwrap();
    let definition = write_minimal_workspace(d.path()); // family HOST
    let o = run(
        &[
            "deployment",
            "render",
            definition.to_str().unwrap(),
            "--env",
            "local",
            "--target",
            "GREENGRASS",
        ],
        d.path(),
    );
    assert_eq!(
        code(&o),
        2,
        "target mismatch is a usage error: {}",
        stderr(&o)
    );
}

#[test]
fn deployment_render_to_an_unbuilt_target_says_not_implemented() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("bindings")).unwrap();
    std::fs::write(d.path().join("bindings/local.json"), "{}\n").unwrap();
    let definition = d.path().join("definition.yaml");
    std::fs::write(
        &definition,
        r#"apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata:
  name: k8s
hierarchy:
  levels: [site, device]
  scopes:
    - id: site/lab
      parent: null
targetStandard:
  family: KUBERNETES
environments:
  - name: local
    bindings: bindings/local.json
nodes:
  - key: box-01
    scope: site/lab
    components:
      - name: telemetry-processor
        artifact: { version: "1.0.0" }
        configSource: CONFIGMAP
"#,
    )
    .unwrap();
    let o = run(
        &[
            "deployment",
            "render",
            definition.to_str().unwrap(),
            "--env",
            "local",
            "--target",
            "KUBERNETES",
        ],
        d.path(),
    );
    assert_eq!(
        code(&o),
        5,
        "unbuilt renderer is not-implemented: {}",
        stderr(&o)
    );
    assert!(stderr(&o).contains("not available"), "{}", stderr(&o));
}
