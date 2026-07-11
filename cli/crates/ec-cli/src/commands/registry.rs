//! `edgecommons registry` (DESIGN-cli §9).
//!
//! Three layers, not one (D-CLI-11):
//!
//! * **Discovery** — `components.json`: what exists. Human-curated, slow-moving.
//! * **Release index** — `releases/<component>.json`: every published release with per-platform
//!   artifact coordinates and digests. **CI-generated, never hand-edited** (RM-013).
//! * **Pin + lock** — the deployment definition pins a version; `deployment lock` records the digest.
//!
//! `version`/`digest` are deliberately **not** fields on a catalog entry: per-release data in a
//! hand-edited file is stale by the second release, cannot express a historical pin, and one
//! top-level digest is meaningless when a component ships a different artifact per platform.

use std::path::Path;
use std::process::Command as Proc;

use ec_adapters::which;
use ec_diag::{Fatal, Outcome, Report};
use serde_json::Value;

use crate::cli::{Category, Language, RegistryListArgs};

const DEFAULT_REPO: &str = "edgecommons/registry";
const DEFAULT_PATH: &str = "components.json";

/// Load the catalog: an explicit source (URL or local path), or the private registry via `gh`.
fn load_catalog(source: Option<&str>) -> Result<Value, Fatal> {
    match source {
        Some(s) if Path::new(s).is_file() => {
            let text = std::fs::read_to_string(s).map_err(|e| Fatal::Internal(e.to_string()))?;
            serde_json::from_str(&text).map_err(|e| Fatal::Internal(format!("{s}: {e}")))
        }
        Some(s) if s.starts_with("http://") || s.starts_with("https://") => {
            Err(Fatal::Environment(format!(
                "fetching a registry over HTTP is not supported by this build ({s}). \
             Pass a local path, or rely on the default `gh`-authenticated read."
            )))
        }
        Some(s) => Err(Fatal::Usage(format!("no such registry file: {s}"))),
        None => load_via_gh(),
    }
}

/// The default: read the (private) registry through an authenticated `gh`.
///
/// Shelling out to `gh` rather than embedding an HTTP client and a token store is deliberate —
/// it keeps credentials out of this binary entirely.
fn load_via_gh() -> Result<Value, Fatal> {
    if which("gh").is_none() {
        return Err(Fatal::Environment(
            "gh not found on PATH — needed to read the private edgecommons registry. \
             Install it, or pass --source <path|url>."
                .into(),
        ));
    }
    let out = Proc::new("gh")
        .args([
            "api",
            &format!("repos/{DEFAULT_REPO}/contents/{DEFAULT_PATH}?ref=main"),
            "-H",
            "Accept: application/vnd.github.raw",
        ])
        .output()
        .map_err(|e| Fatal::Environment(format!("gh failed: {e}")))?;

    if !out.status.success() {
        return Err(Fatal::Environment(format!(
            "gh could not read {DEFAULT_REPO}/{DEFAULT_PATH}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| Fatal::Internal(format!("the registry is not valid JSON: {e}")))
}

pub fn list(args: &RegistryListArgs, json: bool) -> Outcome {
    let catalog = load_catalog(args.source.as_deref())?;
    let components = catalog
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| Fatal::Internal("the registry has no `components` array".into()))?;

    let matched: Vec<&Value> = components
        .iter()
        .filter(|c| matches_language(c, args.language))
        .filter(|c| matches_category(c, args.category))
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&matched).unwrap_or_default()
        );
        return Ok(Report::new());
    }

    if matched.is_empty() {
        println!("No components matched.");
        return Ok(Report::new());
    }

    let w = |k: &str| matched.iter().map(|c| field(c, k).len()).max().unwrap_or(0);
    let (nw, lw, cw) = (w("name"), w("language"), w("category"));
    println!("edgecommons components ({})\n", matched.len());
    for c in &matched {
        println!(
            "  {:<nw$}  {:<lw$}  {:<cw$}  {}",
            field(c, "name"),
            field(c, "language"),
            field(c, "category"),
            field(c, "description"),
        );
    }
    Ok(Report::new())
}

pub fn show(name: &str, source: Option<&str>, json: bool) -> Outcome {
    let catalog = load_catalog(source)?;
    let empty = Vec::new();
    let components = catalog
        .get("components")
        .and_then(Value::as_array)
        .unwrap_or(&empty);

    let Some(c) = components.iter().find(|c| field(c, "name") == name) else {
        return Err(Fatal::Usage(format!(
            "no component `{name}` in the registry"
        )));
    };

    if json {
        println!("{}", serde_json::to_string_pretty(c).unwrap_or_default());
    } else {
        println!("{}\n", field(c, "name"));
        for key in [
            "language",
            "category",
            "protocol",
            "status",
            "library",
            "repo",
            "description",
        ] {
            let v = field(c, key);
            if !v.is_empty() {
                println!("  {key:<12} {v}");
            }
        }
        if let Some(p) = c.get("platforms").and_then(Value::as_array) {
            let ps: Vec<String> = p
                .iter()
                .map(|x| x.as_str().unwrap_or_default().to_string())
                .collect();
            println!("  {:<12} {}", "platforms", ps.join(", "));
        }
    }
    Ok(Report::new())
}

/// `registry versions` — reads the **release index** (RM-013), which does not exist yet.
///
/// It reports that honestly rather than inventing versions. No EdgeCommons component publishes
/// anything today: zero releases, zero tags, zero packages across all eight repos.
pub fn versions(name: &str, source: Option<&str>) -> Outcome {
    // The component must at least exist in the catalog before we talk about its releases.
    let catalog = load_catalog(source)?;
    let empty = Vec::new();
    let components = catalog
        .get("components")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if !components.iter().any(|c| field(c, "name") == name) {
        return Err(Fatal::Usage(format!(
            "no component `{name}` in the registry"
        )));
    }

    let mut r = Report::new();
    r.push(
        ec_diag::Diagnostic::warning(
            ec_diag::Code("EC5001"),
            format!(
                "`{name}` has no published releases: the registry carries no release index yet"
            ),
        )
        .with_help(
            "component release engineering is roadmap RM-013 — until it lands, a deployment \
             definition must hand-pin a version and its digest cannot be verified",
        ),
    );
    Ok(r)
}

fn field(c: &Value, key: &str) -> String {
    c.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn matches_language(c: &Value, want: Option<Language>) -> bool {
    let Some(want) = want else { return true };
    let have = field(c, "language");
    have.eq_ignore_ascii_case(match want {
        Language::Java => "JAVA",
        Language::Python => "PYTHON",
        Language::Rust => "RUST",
        Language::Typescript => "TYPESCRIPT",
    })
}

fn matches_category(c: &Value, want: Option<Category>) -> bool {
    let Some(want) = want else { return true };
    let have = field(c, "category");
    have.eq_ignore_ascii_case(match want {
        Category::Adapter => "adapter",
        Category::Processor => "processor",
        Category::Sink => "sink",
        Category::Bridge => "bridge",
        Category::Console => "console",
        Category::Service => "service",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog() -> Value {
        json!({
            "schemaVersion": 1,
            "components": [
                { "name": "opcua-adapter", "language": "JAVA", "category": "adapter", "description": "OPC UA", "repo": "edgecommons/opcua-adapter" },
                { "name": "uns-bridge", "language": "RUST", "category": "bridge", "description": "Relay", "repo": "edgecommons/uns-bridge" },
                { "name": "edge-console", "language": "RUST", "category": "console", "description": "UI", "repo": "edgecommons/edge-console" },
                { "name": "config-component", "language": "RUST", "category": "service", "description": "Config", "repo": "edgecommons/config-component" }
            ]
        })
    }

    fn write_catalog() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("components.json"), catalog().to_string()).unwrap();
        d
    }

    #[test]
    fn all_six_categories_filter_correctly() {
        // The Python CLI's help advertised three of the six the schema defines; `bridge`,
        // `console` and `service` were undiscoverable.
        let c = catalog();
        let comps = c["components"].as_array().unwrap();
        for (cat, expect) in [
            (Category::Adapter, 1),
            (Category::Bridge, 1),
            (Category::Console, 1),
            (Category::Service, 1),
            (Category::Processor, 0),
            (Category::Sink, 0),
        ] {
            let n = comps
                .iter()
                .filter(|x| matches_category(x, Some(cat)))
                .count();
            assert_eq!(n, expect, "category {cat:?}");
        }
    }

    #[test]
    fn language_filtering_is_case_insensitive() {
        let c = catalog();
        let comps = c["components"].as_array().unwrap();
        assert_eq!(
            comps
                .iter()
                .filter(|x| matches_language(x, Some(Language::Rust)))
                .count(),
            3
        );
        assert_eq!(
            comps
                .iter()
                .filter(|x| matches_language(x, Some(Language::Java)))
                .count(),
            1
        );
        assert_eq!(
            comps.iter().filter(|x| matches_language(x, None)).count(),
            4
        );
    }

    #[test]
    fn a_local_catalog_loads() {
        let d = write_catalog();
        let p = d.path().join("components.json");
        let loaded = load_catalog(Some(p.to_str().unwrap())).unwrap();
        assert_eq!(loaded["components"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn a_missing_catalog_is_a_usage_error_not_a_crash() {
        assert!(matches!(
            load_catalog(Some("/no/such/file.json")),
            Err(Fatal::Usage(_))
        ));
    }

    #[test]
    fn versions_reports_the_missing_release_index_rather_than_inventing_one() {
        let d = write_catalog();
        let p = d.path().join("components.json");
        let r = versions("uns-bridge", Some(p.to_str().unwrap())).unwrap();
        assert_eq!(r.warning_count(), 1);
        assert_eq!(
            r.error_count(),
            0,
            "a missing release index warns; it does not fail"
        );
        assert!(r.diagnostics[0].message.contains("no published releases"));
    }

    #[test]
    fn versions_of_an_unknown_component_is_a_usage_error() {
        let d = write_catalog();
        let p = d.path().join("components.json");
        assert!(matches!(
            versions("nope", Some(p.to_str().unwrap())),
            Err(Fatal::Usage(_))
        ));
    }
}
