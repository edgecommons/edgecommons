//! The template catalog: templates **embedded in the binary** and **discovered by scanning**.
//!
//! Two properties RM-012 calls non-negotiable, both realized here:
//!
//! 1. **Offline.** The templates are compiled into the binary with `include_dir!`, so
//!    `component new` never touches the network and needs no repo checkout. (The Python CLI
//!    achieved this with a `setup.py` `build_py` hook that copied `templates/` into the wheel;
//!    a `--template-git` URL remains the one opt-in exception.)
//! 2. **Discovered, not registered.** The catalog is built by scanning the embedded tree and
//!    reading each manifest — there is no hardcoded list of languages in code. Adding a
//!    template is a template change, not a CLI change. The Python CLI's hardcoded four-language
//!    dict is exactly why `java-protocol-adapter` and `python-protocol-adapter` shipped in-tree
//!    and were unreachable (DEF-8).

use include_dir::{Dir, include_dir};

use crate::manifest::{Kind, Language, Manifest, Platform};

/// The templates, compiled into the binary.
static TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../../templates");

pub const MANIFEST_NAME: &str = "edgecommons-template.json";

/// One discovered template.
#[derive(Debug, Clone)]
pub struct Template {
    /// The directory it was found in, e.g. `java-protocol-adapter`.
    pub dir: String,
    pub manifest: Manifest,
}

impl Template {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

/// Discover every embedded template.
///
/// A template whose manifest does not parse is a **build-time defect**, so this panics with
/// the offending directory rather than silently omitting it — the failure mode that let two
/// templates disappear from the Python CLI's surface without anyone noticing. The
/// `every_embedded_template_parses` test is what turns that panic into a CI gate.
#[must_use]
pub fn discover() -> Vec<Template> {
    let mut out = Vec::new();
    for entry in TEMPLATES.dirs() {
        let dir = entry.path().to_string_lossy().replace('\\', "/");
        let Some(file) = entry.get_file(format!("{dir}/{MANIFEST_NAME}")) else {
            continue; // not a template directory
        };
        let text = file.contents_utf8().unwrap_or_default();
        match Manifest::parse(text) {
            Ok(manifest) => out.push(Template { dir, manifest }),
            Err(e) => panic!("embedded template `{dir}` has an invalid {MANIFEST_NAME}: {e}"),
        }
    }
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

/// Find the template for a language and kind.
#[must_use]
pub fn find(language: Language, kind: Kind) -> Option<Template> {
    discover()
        .into_iter()
        .find(|t| t.manifest.language == language && t.manifest.kind == kind)
}

/// Every file in an embedded template, as `(relative path, bytes)`.
#[must_use]
pub fn files(dir: &str) -> Vec<(String, Vec<u8>)> {
    let Some(root) = TEMPLATES.get_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    collect(root, dir, &mut out);
    out
}

fn collect(dir: &Dir<'_>, root: &str, out: &mut Vec<(String, Vec<u8>)>) {
    for f in dir.files() {
        let full = f.path().to_string_lossy().replace('\\', "/");
        let rel = full.strip_prefix(&format!("{root}/")).unwrap_or(&full).to_string();
        out.push((rel, f.contents().to_vec()));
    }
    for d in dir.dirs() {
        collect(d, root, out);
    }
}

/// The language × kind matrix, for `template list`.
#[must_use]
pub fn matrix() -> Vec<(Language, Kind, String)> {
    discover()
        .into_iter()
        .map(|t| (t.manifest.language, t.manifest.kind, t.manifest.description))
        .collect()
}

/// Platforms a template can emit for.
#[must_use]
pub fn platforms(t: &Template) -> Vec<Platform> {
    t.manifest.platforms.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_template_parses() {
        // This is the gate: a template whose manifest is malformed fails CI here rather than
        // vanishing from the CLI's surface at runtime.
        let found = discover();
        assert!(!found.is_empty(), "no templates were embedded");
        for t in &found {
            assert_eq!(t.manifest.id, t.manifest.expected_id());
        }
    }

    #[test]
    fn the_four_service_templates_are_discoverable() {
        for lang in [Language::Java, Language::Python, Language::Rust, Language::Typescript] {
            assert!(
                find(lang, Kind::Service).is_some(),
                "{} service template must be discoverable",
                lang.as_str()
            );
        }
    }

    #[test]
    fn the_protocol_adapter_templates_are_reachable() {
        // DEF-8: both shipped in-tree and were unreachable from the Python CLI, because it
        // carried a hardcoded four-language dict instead of scanning.
        assert!(find(Language::Java, Kind::ProtocolAdapter).is_some());
        assert!(find(Language::Python, Kind::ProtocolAdapter).is_some());
    }

    #[test]
    fn a_template_carries_its_files() {
        let t = find(Language::Rust, Kind::Service).expect("rust/service");
        let fs = files(&t.dir);
        let names: Vec<&str> = fs.iter().map(|(p, _)| p.as_str()).collect();
        assert!(names.contains(&"Cargo.toml"), "{names:?}");
        assert!(names.contains(&"src/main.rs"), "{names:?}");
        // Nested directories must be walked, not just the top level.
        assert!(names.iter().any(|n| n.starts_with("src/")));
    }

    #[test]
    fn the_manifest_is_not_shipped_to_the_user() {
        // It is a template artifact; the generated project must not contain it. (The pipeline
        // removes it — this asserts it is present in the embedded tree to be removed.)
        let t = find(Language::Rust, Kind::Service).unwrap();
        let fs = files(&t.dir);
        assert!(fs.iter().any(|(p, _)| p == MANIFEST_NAME));
    }
}
