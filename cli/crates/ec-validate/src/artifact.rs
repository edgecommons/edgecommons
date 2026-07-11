//! Layer 3 — artifact lint (DESIGN-cli §6.3).
//!
//! The Python `recipe_lint` never parsed the recipe: it ran three regexes over the *text*. So
//! `Permissions:` inside a comment or a string tripped it, a genuinely malformed recipe passed,
//! and `gdk-config.json` was never looked at at all. This parses.
//!
//! It also wires in the `RequiresPrivilege` check, which existed in the Python CLI as
//! `lint_least_privilege` and **was never called by `validate`** — only by a test (DEF-9).

use std::path::Path;

use ec_diag::{Diagnostic, Report};
use serde::Deserialize;
use serde_json::Value as Json;
use serde_yaml::Value as Yaml;

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
