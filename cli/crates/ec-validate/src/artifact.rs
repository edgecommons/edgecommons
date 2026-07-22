//! Layer 3 — artifact lint (DESIGN-cli §6.3).
//!
//! The Python `recipe_lint` never parsed the recipe: it ran three regexes over the *text*. So
//! `Permissions:` inside a comment or a string tripped it, a genuinely malformed recipe passed,
//! and `gdk-config.json` was never looked at at all. This parses.
//!
//! It also wires in the `RequiresPrivilege` check, which existed in the Python CLI as
//! `lint_least_privilege` and **was never called by `validate`** — only by a test (DEF-9).

use std::path::{Path, PathBuf};

use ec_diag::{Diagnostic, Report};
use serde::Deserialize;
use serde_json::Value as Json;
use serde_yaml::Value as Yaml;

/// The sentinel `component new` writes into a Greengrass `gdk-config.json` publish bucket when
/// none was supplied. `component validate` errors on it, so the miss is caught at authoring/CI
/// rather than at `gdk component publish`. Kept in sync with the CLI's own constant by this test
/// suite's `the_bucket_sentinel_is_caught` case.
pub const ARTIFACT_BUCKET_SENTINEL: &str = "edgecommons-set-artifact-bucket";

/// Lint a Greengrass recipe.
#[must_use]
pub fn lint_recipe(path: &Path) -> Report {
    let mut r = Report::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return r; // no recipe is not an error; a component need not target Greengrass
    };

    // Unsubstituted tokens are a text-level fact, and worth catching even in a recipe that
    // does not parse.
    for (i, line) in text.lines().enumerate() {
        if line.contains("<<") && line.contains(">>") {
            r.push(
                Diagnostic::error(ec_diag::EC3003_UNSUBSTITUTED_TOKEN, line.trim().to_string())
                    .with_file(path)
                    .with_line(i + 1),
            );
        }
    }

    let doc: Yaml = match serde_yaml::from_str(&text) {
        Ok(d) => d,
        Err(e) => {
            r.push(
                Diagnostic::error(
                    ec_diag::EC3005_RECIPE_UNPARSABLE,
                    format!("recipe is not valid YAML: {e}"),
                )
                .with_file(path),
            );
            return r;
        }
    };

    // GDK does not substitute {COMPONENT_NAME}; `gdk component publish` rejects the recipe.
    if let Some(name) = doc.get("ComponentName").and_then(|v| v.as_str())
        && name.contains("{COMPONENT_NAME}")
    {
        r.push(
            Diagnostic::error(
                ec_diag::EC3001_RECIPE_COMPONENT_NAME_PLACEHOLDER,
                "ComponentName uses the `{COMPONENT_NAME}` placeholder, which GDK does not substitute"
                    .to_string(),
            )
            .with_file(path)
            .with_help("use the literal component name"),
        );
    }

    // Walk the manifests: an artifact `Permissions:` block is rejected by
    // CreateComponentVersion, and `RequiresPrivilege: true` runs the component as root.
    if let Some(manifests) = doc.get("Manifests").and_then(|v| v.as_sequence()) {
        for m in manifests {
            if let Some(artifacts) = m.get("Artifacts").and_then(|v| v.as_sequence()) {
                for a in artifacts {
                    if a.get("Permissions").is_some() {
                        r.push(
                            Diagnostic::error(
                                ec_diag::EC3002_RECIPE_PERMISSIONS_BLOCK,
                                "an artifact `Permissions:` block is present; CreateComponentVersion rejects it"
                                    .to_string(),
                            )
                            .with_file(path)
                            .with_help("remove it and make artifacts executable from an Install lifecycle (chmod)"),
                        );
                    }
                }
            }
            r.extend(requires_privilege(m, path));
        }
    }

    r
}

/// `RequiresPrivilege: true` runs the component as root.
///
/// A **warning**, not an error: it is occasionally legitimate. But it must actually be
/// reported — in the Python CLI this check existed and `validate` never called it.
fn requires_privilege(node: &Yaml, path: &Path) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        match n {
            Yaml::Mapping(map) => {
                for (k, v) in map {
                    if k.as_str() == Some("RequiresPrivilege") && v.as_bool() == Some(true) {
                        out.push(
                            Diagnostic::warning(
                                ec_diag::EC3004_REQUIRES_PRIVILEGE,
                                "`RequiresPrivilege: true` runs this component as root".to_string(),
                            )
                            .with_file(path)
                            .with_help(
                                "rarely needed — Greengrass IPC, TES, and the ggc_user work dir all \
                                 work unprivileged; prefer least privilege",
                            ),
                        );
                    }
                    stack.push(v);
                }
            }
            Yaml::Sequence(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    out
}

/// `EC2003` — a Kubernetes ConfigMap mount must not use `subPath`.
///
/// A `subPath` mount is resolved once, at pod start: the kubelet's `..data` symlink swap never
/// reaches it, so the config **silently stops hot-reloading**. The component keeps running on
/// stale config and nothing reports it — which is precisely the failure the whole config-source
/// design exists to avoid.
#[must_use]
pub fn lint_k8s(dir: &Path) -> Report {
    let mut r = Report::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return r; // no k8s pack: not a Kubernetes component
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for doc in serde_yaml::Deserializer::from_str(&text) {
            let Ok(doc) = Yaml::deserialize(doc) else {
                r.push(
                    Diagnostic::error(
                        ec_diag::EC3005_RECIPE_UNPARSABLE,
                        "manifest is not valid YAML".to_string(),
                    )
                    .with_file(&path),
                );
                continue;
            };
            if has_subpath_mount(&doc) {
                r.push(
                    Diagnostic::error(
                        ec_diag::EC2003_CONFIGMAP_SUBPATH,
                        "a volume mount uses `subPath`, which breaks ConfigMap hot-reload".to_string(),
                    )
                    .with_file(&path)
                    .with_help(
                        "mount the whole ConfigMap volume; a subPath mount never sees the kubelet's \
                         ..data swap, so the component silently keeps running on stale config",
                    ),
                );
            }
        }
    }
    r
}

fn has_subpath_mount(node: &Yaml) -> bool {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        match n {
            Yaml::Mapping(map) => {
                if map.keys().any(|k| k.as_str() == Some("subPath")) {
                    return true;
                }
                stack.extend(map.values());
            }
            Yaml::Sequence(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    false
}

/// The component configs **embedded in deployment artifacts**, with the artifact they came from.
///
/// A component ships its config in three places, not one: `test-configs/` (what you run locally),
/// the Kubernetes ConfigMap, and the Greengrass recipe's `DefaultConfiguration`. Validating only
/// the first is validating the one that fails cheapest. The deployed defaults are the ones that
/// fail at 3am on a device you cannot reach — so they are validated too, against the same schema.
#[must_use]
pub fn embedded_configs(root: &Path) -> Vec<(PathBuf, Json)> {
    let mut out = Vec::new();

    // The Kubernetes ConfigMap: `data["config.json"]` is a JSON document inside a YAML string.
    let cm = root.join("k8s").join("configmap.yaml");
    if let Ok(text) = std::fs::read_to_string(&cm)
        && let Ok(doc) = serde_yaml::from_str::<Yaml>(&text)
        && let Some(raw) = doc
            .get("data")
            .and_then(|d| d.get("config.json"))
            .and_then(|v| v.as_str())
        && let Ok(cfg) = serde_json::from_str::<Json>(raw)
    {
        out.push((cm, cfg));
    }

    // The Greengrass recipe's deployed default configuration.
    let recipe = root.join("recipe.yaml");
    if let Ok(text) = std::fs::read_to_string(&recipe)
        && let Ok(doc) = serde_yaml::from_str::<Yaml>(&text)
        && let Some(cfg) = doc
            .get("ComponentConfiguration")
            .and_then(|c| c.get("DefaultConfiguration"))
            .and_then(|c| c.get("ComponentConfig"))
        && let Ok(json) = serde_json::to_value(cfg)
    {
        out.push((recipe, json));
    }

    out
}

/// Lint `gdk-config.json`, which the Python CLI never looked at.
#[must_use]
pub fn lint_gdk_config(path: &Path) -> Report {
    let mut r = Report::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return r;
    };

    let doc: Json = match serde_json::from_str(&text) {
        Ok(d) => d,
        Err(e) => {
            r.push(
                Diagnostic::error(
                    ec_diag::EC3006_GDK_CONFIG,
                    format!("gdk-config.json is not valid JSON: {e}"),
                )
                .with_file(path),
            );
            return r;
        }
    };

    let Some(component) = doc.get("component").and_then(Json::as_object) else {
        r.push(
            Diagnostic::error(
                ec_diag::EC3006_GDK_CONFIG,
                "gdk-config.json has no `component` object".to_string(),
            )
            .with_file(path),
        );
        return r;
    };

    if component.is_empty() {
        r.push(
            Diagnostic::error(
                ec_diag::EC3006_GDK_CONFIG,
                "gdk-config.json declares no component".to_string(),
            )
            .with_file(path),
        );
    }

    // The publish bucket is still the scaffold sentinel: an unresolved bucket that would fail at
    // `gdk component publish`. Caught here as an error so it surfaces at authoring/CI instead.
    for (_, body) in component {
        if body
            .get("publish")
            .and_then(|p| p.get("bucket"))
            .and_then(Json::as_str)
            == Some(ARTIFACT_BUCKET_SENTINEL)
        {
            r.push(
                Diagnostic::error(
                    ec_diag::EC3007_ARTIFACT_BUCKET_SENTINEL,
                    format!(
                        "gdk-config.json publish bucket is the sentinel \
                         `{ARTIFACT_BUCKET_SENTINEL}` and cannot publish"
                    ),
                )
                .with_file(path)
                .with_help("set `publish.bucket` to a real S3 bucket you own"),
            );
        }
    }

    r
}

/// Warn when a Rust or TypeScript component ships no committed lockfile (SD-6). A template
/// cannot ship a valid lockfile (it depends on the dep-source and the resolution moment), so the
/// policy is instruct-and-validate: `component new` prints a "commit the lockfile" next step, and
/// this rule warns until the author has done so. A warning, not an error — the very first
/// `component validate`, before any build, is expected to hit it.
#[must_use]
pub fn lint_lockfile(root: &Path) -> Report {
    let mut r = Report::new();
    for (manifest, lockfile) in [
        ("Cargo.toml", "Cargo.lock"),
        ("package.json", "package-lock.json"),
    ] {
        if root.join(manifest).exists() && !root.join(lockfile).exists() {
            r.push(
                Diagnostic::warning(
                    ec_diag::EC4008_NO_LOCKFILE,
                    format!("no committed {lockfile}: builds are not reproducible"),
                )
                .with_file(root.join(manifest))
                .with_help(format!(
                    "build once and commit {lockfile} so CI resolves the same dependency graph"
                )),
            );
        }
    }
    r
}

/// Whether a `gdk-config.json` pins a concrete version, or leaves `NEXT_PATCH`.
///
/// This is the ancestor of the release-lock gate: a deployment must consume a concrete
/// version, not a moving target. It is **not** a lint error on a scaffold — every template
/// ships `NEXT_PATCH`, which is the correct GDK idiom for "let publish pick the next one" —
/// so it is reported only where it matters, at release/deploy time.
#[must_use]
pub fn declared_version(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc: Json = serde_json::from_str(&text).ok()?;
    let component = doc.get("component")?.as_object()?;
    let (_, body) = component.iter().next()?;
    let v = body.get("version")?.as_str()?;
    Some(v.to_string())
}

/// Is this a concrete, deployable version?
#[must_use]
pub fn is_locked_version(v: &str) -> bool {
    !v.is_empty() && v != "NEXT_PATCH"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn a_clean_recipe_passes() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "recipe.yaml",
            r"
RecipeFormatVersion: '2020-01-25'
ComponentName: com.example.Thing
ComponentVersion: '1.0.0'
Manifests:
  - Platform:
      os: linux
    Lifecycle:
      Run:
        Script: ./thing
",
        );
        let r = lint_recipe(&p);
        assert_eq!(r.error_count(), 0, "{}", r.render_human());
        assert_eq!(r.warning_count(), 0);
    }

    #[test]
    fn the_component_name_placeholder_is_caught() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "recipe.yaml",
            "ComponentName: '{COMPONENT_NAME}'\nComponentVersion: '1.0.0'\n",
        );
        let r = lint_recipe(&p);
        assert_eq!(r.error_count(), 1);
        assert_eq!(
            r.diagnostics[0].code,
            ec_diag::EC3001_RECIPE_COMPONENT_NAME_PLACEHOLDER
        );
    }

    #[test]
    fn an_artifact_permissions_block_is_caught() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "recipe.yaml",
            r"
ComponentName: com.example.Thing
Manifests:
  - Artifacts:
      - URI: s3://bucket/thing.zip
        Permissions:
          Execute: OWNER
",
        );
        let r = lint_recipe(&p);
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(
            r.diagnostics[0].code,
            ec_diag::EC3002_RECIPE_PERMISSIONS_BLOCK
        );
    }

    #[test]
    fn parsing_means_a_comment_no_longer_trips_the_permissions_rule() {
        // The Python lint regexed the text, so this comment was a false positive.
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "recipe.yaml",
            r"
ComponentName: com.example.Thing
# Permissions: are set from the Install lifecycle instead.
Manifests:
  - Lifecycle:
      Run:
        Script: ./thing
",
        );
        let r = lint_recipe(&p);
        assert_eq!(
            r.error_count(),
            0,
            "a comment must not trip the rule: {}",
            r.render_human()
        );
    }

    #[test]
    fn requires_privilege_warns_and_is_actually_wired_in() {
        // DEF-9: this check existed in the Python CLI and `validate` never called it.
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "recipe.yaml",
            r"
ComponentName: com.example.Thing
Manifests:
  - Lifecycle:
      Run:
        RequiresPrivilege: true
        Script: ./thing
",
        );
        let r = lint_recipe(&p);
        assert_eq!(r.warning_count(), 1, "{}", r.render_human());
        assert_eq!(
            r.error_count(),
            0,
            "root is a warning, not an error — it is occasionally legitimate"
        );
        assert_eq!(r.diagnostics[0].code, ec_diag::EC3004_REQUIRES_PRIVILEGE);
    }

    #[test]
    fn a_malformed_recipe_is_reported_rather_than_passing_silently() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "recipe.yaml", "ComponentName: [unclosed\n");
        let r = lint_recipe(&p);
        assert_eq!(r.error_count(), 1);
        assert_eq!(r.diagnostics[0].code, ec_diag::EC3005_RECIPE_UNPARSABLE);
    }

    #[test]
    fn leftover_tokens_are_caught_with_a_line_number() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "recipe.yaml",
            "ComponentName: com.example.Thing\nAuthor: <<AUTHOR>>\n",
        );
        let r = lint_recipe(&p);
        assert!(
            r.diagnostics
                .iter()
                .any(|x| x.code == ec_diag::EC3003_UNSUBSTITUTED_TOKEN)
        );
    }

    #[test]
    fn a_missing_recipe_is_not_an_error() {
        // A HOST-only or Kubernetes-only component ships no recipe, and that is correct.
        let d = tempfile::tempdir().unwrap();
        let r = lint_recipe(&d.path().join("recipe.yaml"));
        assert!(r.is_empty());
    }

    #[test]
    fn a_configmap_subpath_mount_is_caught() {
        // EC2003: a subPath mount is resolved once at pod start, so the kubelet's ..data swap
        // never reaches it and the config SILENTLY stops hot-reloading. The component keeps
        // running on stale config and nothing says a word.
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "k8s/deployment.yaml",
            r"
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: thing
          volumeMounts:
            - name: config
              mountPath: /etc/edgecommons/config.json
              subPath: config.json
",
        );
        let r = lint_k8s(&d.path().join("k8s"));
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC2003_CONFIGMAP_SUBPATH);
    }

    #[test]
    fn a_whole_volume_configmap_mount_is_clean() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "k8s/deployment.yaml",
            r"
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers:
        - name: thing
          volumeMounts:
            - name: config
              mountPath: /etc/edgecommons
",
        );
        assert_eq!(lint_k8s(&d.path().join("k8s")).error_count(), 0);
    }

    #[test]
    fn a_component_with_no_k8s_pack_is_not_linted_for_it() {
        let d = tempfile::tempdir().unwrap();
        assert!(lint_k8s(&d.path().join("k8s")).is_empty());
    }

    #[test]
    fn gdk_config_is_actually_parsed_now() {
        let d = tempfile::tempdir().unwrap();
        let bad = write(d.path(), "gdk-config.json", "{ not json");
        let r = lint_gdk_config(&bad);
        assert_eq!(r.error_count(), 1);
        assert_eq!(r.diagnostics[0].code, ec_diag::EC3006_GDK_CONFIG);

        let ok = write(
            d.path(),
            "ok/gdk-config.json",
            r#"{"component":{"com.example.Thing":{"version":"NEXT_PATCH"}},"gdk_version":"1.6.2"}"#,
        );
        assert_eq!(lint_gdk_config(&ok).error_count(), 0);
    }

    #[test]
    fn the_bucket_sentinel_is_caught() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "gdk-config.json",
            &format!(
                r#"{{"component":{{"com.example.Thing":{{"version":"NEXT_PATCH","publish":{{"bucket":"{ARTIFACT_BUCKET_SENTINEL}","region":"us-east-1"}}}}}}}}"#
            ),
        );
        let r = lint_gdk_config(&p);
        assert_eq!(r.error_count(), 1, "{}", r.render_human());
        assert_eq!(
            r.diagnostics[0].code,
            ec_diag::EC3007_ARTIFACT_BUCKET_SENTINEL
        );

        // A real bucket is clean.
        let ok = write(
            d.path(),
            "ok/gdk-config.json",
            r#"{"component":{"com.example.Thing":{"publish":{"bucket":"my-real-bucket"}}}}"#,
        );
        assert_eq!(lint_gdk_config(&ok).error_count(), 0);
    }

    #[test]
    fn a_rust_project_without_a_committed_lockfile_warns() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "Cargo.toml", "[package]\nname=\"x\"\n");
        let r = lint_lockfile(d.path());
        assert_eq!(r.warning_count(), 1, "{}", r.render_human());
        assert_eq!(
            r.error_count(),
            0,
            "a missing lockfile is a warning, not an error"
        );
        assert_eq!(r.diagnostics[0].code, ec_diag::EC4008_NO_LOCKFILE);

        // Once the lockfile is committed, it is clean.
        write(d.path(), "Cargo.lock", "# lock\n");
        assert!(lint_lockfile(d.path()).is_empty());
    }

    #[test]
    fn a_typescript_project_without_a_committed_lockfile_warns() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "package.json", "{\"name\":\"x\"}\n");
        let r = lint_lockfile(d.path());
        assert_eq!(r.warning_count(), 1, "{}", r.render_human());
        assert_eq!(r.diagnostics[0].code, ec_diag::EC4008_NO_LOCKFILE);
        write(d.path(), "package-lock.json", "{}\n");
        assert!(lint_lockfile(d.path()).is_empty());
    }

    #[test]
    fn a_project_with_no_rust_or_ts_manifest_is_not_lockfile_linted() {
        // A Java or Python component has no lockfile convention this rule covers.
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "pom.xml", "<project/>\n");
        write(d.path(), "requirements.txt", "edgecommons\n");
        assert!(lint_lockfile(d.path()).is_empty());
    }

    #[test]
    fn version_locking_distinguishes_next_patch_from_a_real_version() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "gdk-config.json",
            r#"{"component":{"com.example.Thing":{"version":"NEXT_PATCH"}}}"#,
        );
        assert_eq!(declared_version(&p).as_deref(), Some("NEXT_PATCH"));
        assert!(!is_locked_version("NEXT_PATCH"));
        assert!(is_locked_version("1.4.2"));
        assert!(!is_locked_version(""));
    }
}
