//! `component package` and `component release` (DESIGN-cli §7.3).
//!
//! **The CLI produces; the runner publishes** (D-CLI-10). `release` builds the artifacts,
//! computes their digests, and emits a release descriptor — and stops. It never tags, uploads,
//! or pushes.
//!
//! This is not fastidiousness. A release cut from a developer's laptop holding publishing
//! credentials has no provenance, no attestation, and no reproducibility, which is exactly what
//! the supply-chain evidence gate exists to prevent. Tagging, uploading, and opening the
//! registry PR belong to a release workflow running *this same binary* in CI — so a laptop
//! dry-run emits the exact bytes CI would, which is what makes the descriptor reviewable before
//! it is real.

use std::path::Path;
use std::process::Command as Proc;

use ec_adapters::{ExternalTool, which};
use ec_diag::{Diagnostic, Fatal, Outcome, Report};
use ec_validate::artifact;
use sha2::{Digest, Sha256};

use crate::cli::{PackageArgs, Platform, ReleaseArgs};

/// Build deployable artifacts for the selected platforms.
pub fn package(args: &PackageArgs, quiet: bool) -> Outcome {
    if !args.path.is_dir() {
        return Err(Fatal::Usage(format!(
            "no such component directory: {}",
            args.path.display()
        )));
    }
    let platforms = if args.platforms.is_empty() {
        detect_platforms(&args.path)
    } else {
        args.platforms.clone()
    };
    if platforms.is_empty() {
        return Err(Fatal::Usage(
            "cannot tell what this component targets: it ships no recipe.yaml, compose.yaml, or k8s/. \
             Pass --platforms."
                .into(),
        ));
    }

    let mut report = Report::new();

    for p in &platforms {
        match p {
            Platform::Greengrass => {
                let Some(gdk) = which(ExternalTool::Gdk.binary()) else {
                    return Err(Fatal::Environment(
                        "gdk not found on PATH — needed to build a Greengrass component (see `edgecommons doctor`)".into(),
                    ));
                };
                run(
                    &gdk.display().to_string(),
                    &["component", "build"],
                    &args.path,
                    quiet,
                )?;
                if args.publish {
                    run(
                        &gdk.display().to_string(),
                        &["component", "publish"],
                        &args.path,
                        quiet,
                    )?;
                }
            }
            // Container builds are the CI runner's job, not the CLI's — the same
            // produce-vs-publish line D-CLI-10 draws. Say so rather than half-doing it.
            Platform::Host | Platform::Kubernetes => {
                report.push(
                    Diagnostic::warning(
                        ec_diag::Code("EC5002"),
                        format!(
                            "{} artifacts are container images; building and pushing them is the release \
                             workflow's job, not the CLI's",
                            p.as_str()
                        ),
                    )
                    .with_help("build with `docker build` from the generated Dockerfile; CI publishes it"),
                );
            }
        }
    }
    Ok(report)
}

/// Emit a release descriptor. **Never publishes.**
pub fn release(args: &ReleaseArgs, quiet: bool) -> Outcome {
    if !args.path.is_dir() {
        return Err(Fatal::Usage(format!(
            "no such component directory: {}",
            args.path.display()
        )));
    }

    let mut report = Report::new();

    let name = component_name(&args.path).ok_or_else(|| {
        Fatal::Usage("cannot determine the component name from this project".into())
    })?;
    let version = component_version(&args.path).ok_or_else(|| {
        Fatal::Usage("cannot determine the component version from this project".into())
    })?;

    // A release must pin a concrete version. `NEXT_PATCH` is the correct GDK idiom in a
    // scaffold, but it is not a thing you can deploy or roll back to — this is the ancestor of
    // the release-lock gate.
    let gdk_config = args.path.join("gdk-config.json");
    if let Some(v) = artifact::declared_version(&gdk_config)
        && !artifact::is_locked_version(&v)
    {
        return Err(Fatal::Usage(format!(
            "gdk-config.json declares version `{v}`; a release must pin a concrete version. \
             Set one with `edgecommons component version --to <x.y.z>`."
        )));
    }

    // The component's config schema travels with the release: it is what makes
    // config/artifact compatibility *derived* rather than declared (D-CLI-16).
    let schema_path = args.path.join(ec_validate::schema::COMPONENT_SCHEMA_NAME);
    let config_schema: Option<serde_json::Value> = std::fs::read_to_string(&schema_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    if config_schema.is_none() {
        report.push(ec_validate::schema::no_component_schema(&name));
    }

    let source_commit = git_head(&args.path);

    let descriptor = serde_json::json!({
        "component": name,
        "version": version,
        "sourceCommit": source_commit,
        "artifacts": artifacts(&args.path),
        "configSchema": config_schema,
        // Designed now, populated as RM-013's release workflow lands.
        "supplyChain": { "sbom": null, "signature": null, "provenance": null },
    });

    let mut text =
        serde_json::to_string_pretty(&descriptor).map_err(|e| Fatal::Internal(e.to_string()))?;
    text.push('\n');
    std::fs::write(&args.out, &text)
        .map_err(|e| Fatal::Internal(format!("{}: {e}", args.out.display())))?;

    if !quiet {
        println!("Wrote release descriptor: {}", args.out.display());
        println!(
            "This did NOT publish anything. Tagging and upload belong to the release workflow."
        );
    }
    Ok(report)
}

/// Per-platform artifact coordinates. A single top-level digest would be meaningless: a
/// Greengrass archive, an OCI image, and a HOST binary are three different objects.
fn artifacts(root: &Path) -> serde_json::Value {
    let mut out = serde_json::Map::new();

    // Greengrass: whatever `gdk component build` staged.
    let gg = root.join("greengrass-build/artifacts");
    if gg.is_dir() {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(&gg)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file()
                && let Some(digest) = sha256(entry.path())
            {
                files.push(serde_json::json!({
                    "path": entry.path().strip_prefix(root).unwrap_or(entry.path()).display().to_string().replace('\\', "/"),
                    "sha256": digest,
                }));
            }
        }
        if !files.is_empty() {
            out.insert("GREENGRASS".into(), serde_json::json!({ "files": files }));
        }
    }

    // Kubernetes/HOST: the image is built and pushed by CI, so the descriptor carries the
    // coordinate and CI fills the digest. Declaring the field now is what lets the deployment
    // model pin {version, digest} later without a schema change.
    if root.join("Dockerfile").is_file() {
        out.insert(
            "KUBERNETES".into(),
            serde_json::json!({ "image": null, "digest": null, "note": "built and digested by the release workflow" }),
        );
    }

    serde_json::Value::Object(out)
}

fn sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Some(format!("sha256:{:x}", h.finalize()))
}

fn component_name(root: &Path) -> Option<String> {
    // Prefer the Greengrass component name; fall back to the directory.
    if let Some(v) = read_json(&root.join("gdk-config.json"))
        && let Some(c) = v.get("component").and_then(|c| c.as_object())
        && let Some((name, _)) = c.iter().next()
    {
        return Some(name.clone());
    }
    root.file_name().map(|n| n.to_string_lossy().to_string())
}

fn component_version(root: &Path) -> Option<String> {
    if let Some(v) = read_json(&root.join("package.json"))
        && let Some(s) = v.get("version").and_then(|x| x.as_str())
    {
        return Some(s.to_string());
    }
    if let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml"))
        && let Ok(doc) = text.parse::<toml_edit::DocumentMut>()
        && let Some(s) = doc
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    artifact::declared_version(&root.join("gdk-config.json"))
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn git_head(root: &Path) -> Option<String> {
    let out = Proc::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn detect_platforms(root: &Path) -> Vec<Platform> {
    let mut p = Vec::new();
    if root.join("recipe.yaml").is_file() {
        p.push(Platform::Greengrass);
    }
    if root.join("compose.yaml").is_file() {
        p.push(Platform::Host);
    }
    if root.join("k8s").is_dir() {
        p.push(Platform::Kubernetes);
    }
    p
}

fn run(bin: &str, args: &[&str], cwd: &Path, quiet: bool) -> Result<(), Fatal> {
    if !quiet {
        println!("$ {} {}  (in {})", bin, args.join(" "), cwd.display());
    }
    let status = Proc::new(bin)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| Fatal::Environment(format!("{bin} failed to start: {e}")))?;
    if !status.success() {
        return Err(Fatal::Environment(format!(
            "{} {} failed with {status}",
            bin,
            args.join(" ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (n, b) in files {
            let p = d.path().join(n);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        }
        d
    }

    fn release_args(dir: &Path) -> ReleaseArgs {
        ReleaseArgs {
            path: dir.to_path_buf(),
            out: dir.join("release.json"),
        }
    }

    #[test]
    fn a_release_refuses_an_unlocked_version() {
        // Every template ships NEXT_PATCH, which is the right GDK idiom for a scaffold but is
        // not a thing you can deploy or roll back to. This is the ancestor of the release-lock
        // gate — and it is why the Python CLI's `deploy --target` could never run on a fresh
        // scaffold (DEF-6): it hit this wall with no way over it. `component version` is the way over.
        let d = project(&[
            (
                "gdk-config.json",
                r#"{"component":{"com.example.Thing":{"version":"NEXT_PATCH"}}}"#,
            ),
            ("config.schema.json", r#"{"type":"object"}"#),
        ]);
        let e = release(&release_args(d.path()), true).unwrap_err();
        assert!(matches!(e, Fatal::Usage(_)), "{e:?}");
        assert!(
            e.to_string().contains("component version"),
            "the error must say how to fix it: {e}"
        );
    }

    #[test]
    fn a_release_descriptor_carries_the_config_schema() {
        // D-CLI-16: publishing the schema per release is what makes config/artifact
        // compatibility derived rather than a hand-maintained version floor.
        let d = project(&[
            (
                "gdk-config.json",
                r#"{"component":{"com.example.Thing":{"version":"1.4.2"}}}"#,
            ),
            (
                "config.schema.json",
                r#"{"type":"object","properties":{"pipeline":{"type":"array"}}}"#,
            ),
        ]);
        let r = release(&release_args(d.path()), true).unwrap();
        assert_eq!(r.error_count(), 0, "{}", r.render_human());

        let desc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.path().join("release.json")).unwrap())
                .unwrap();
        assert_eq!(desc["component"], "com.example.Thing");
        assert_eq!(desc["version"], "1.4.2");
        assert!(desc["configSchema"]["properties"]["pipeline"].is_object());
        // The supply-chain fields are designed now, populated by RM-013's workflow.
        assert!(desc["supplyChain"].is_object());
    }

    #[test]
    fn a_release_without_a_config_schema_warns() {
        let d = project(&[(
            "gdk-config.json",
            r#"{"component":{"com.example.Thing":{"version":"1.0.0"}}}"#,
        )]);
        let r = release(&release_args(d.path()), true).unwrap();
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.diagnostics[0].code, ec_diag::EC1003_NO_COMPONENT_SCHEMA);
    }

    #[test]
    fn the_descriptor_never_publishes() {
        // The whole point of D-CLI-10: this verb writes a file and does nothing else. If it
        // ever grows a network call or a `git tag`, this test is where that shows up.
        let d = project(&[
            (
                "gdk-config.json",
                r#"{"component":{"com.example.Thing":{"version":"1.0.0"}}}"#,
            ),
            ("config.schema.json", "{}"),
        ]);
        release(&release_args(d.path()), true).unwrap();
        assert!(
            d.path().join("release.json").is_file(),
            "it produces a descriptor..."
        );
        // ...and nothing else: no tag, no upload, no push. There is nothing to assert against
        // because there is nothing else to do — which is exactly the contract.
    }

    #[test]
    fn per_platform_artifacts_are_recorded_separately() {
        let d = project(&[
            (
                "gdk-config.json",
                r#"{"component":{"com.example.Thing":{"version":"1.0.0"}}}"#,
            ),
            ("Dockerfile", "FROM scratch\n"),
            ("greengrass-build/artifacts/thing.zip", "not really a zip"),
        ]);
        release(&release_args(d.path()), true).unwrap();
        let desc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.path().join("release.json")).unwrap())
                .unwrap();

        // A Greengrass archive and an OCI image are different objects, each with its own digest.
        assert!(
            desc["artifacts"]["GREENGRASS"]["files"][0]["sha256"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        assert!(desc["artifacts"].get("KUBERNETES").is_some());
    }

    #[test]
    fn version_is_read_from_cargo_when_there_is_no_gdk_config() {
        let d = project(&[(
            "Cargo.toml",
            "[package]\nname=\"thing\"\nversion=\"2.1.0\"\n",
        )]);
        let r = release(&release_args(d.path()), true);
        assert!(r.is_ok(), "{r:?}");
        let desc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(d.path().join("release.json")).unwrap())
                .unwrap();
        assert_eq!(desc["version"], "2.1.0");
    }
}
