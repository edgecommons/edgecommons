//! The generation pipeline (DESIGN-cli §5.4).
//!
//! Order is load-bearing: copy → prune packs and unmet conditionals → substitute → rename →
//! prune empty dirs → **verify no `<<TOKEN>>` survives**. That last step is a hard error and
//! is kept from the Python CLI deliberately: it is the check that turns template/CLI drift
//! into a failed scaffold rather than a broken project the author discovers at build time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ec_diag::{Diagnostic, Fatal, Report};

use crate::catalog::{self, MANIFEST_NAME, Template};
use crate::manifest::{Language, Platform};

/// The edgecommons library version a generated component depends on.
///
/// Resolved **at CLI build time from the workspace itself** (`libs/rust/Cargo.toml`), never
/// hand-maintained in a constant. The Python CLI hardcoded `_EDGECOMMONS_VERSION = "0.1.0"`,
/// which — once the libraries moved to 0.2.0 — meant `--dep-source registry` emitted a Cargo
/// dependency on the git tag `rust-lib/v0.1.0`, **a tag that does not exist**. The scaffold
/// could not resolve, let alone build (DEF-1). A constant that must be remembered will
/// eventually be forgotten; this one cannot drift.
pub const EDGECOMMONS_VERSION: &str = env!("EC_LIBRARY_VERSION");

const GIT_URL: &str = "https://github.com/edgecommons/edgecommons";

/// Everything `component new` needs.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub full_name: String,
    pub description: String,
    pub author: String,
    pub platforms: Vec<Platform>,
    pub dep_source: DepSource,
    /// Path to a local edgecommons checkout, for `DepSource::Local`.
    pub library_path: Option<PathBuf>,
    /// Greengrass-only; prompted and substituted only when the GREENGRASS pack is selected.
    pub bucket: String,
    pub region: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepSource {
    Local,
    Registry,
}

impl DepSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Registry => "registry",
        }
    }
}

/// The short component name: the last dotted segment of `com.example.MyComponent`.
#[must_use]
pub fn short_name(full: &str) -> String {
    full.rsplit('.').next().unwrap_or(full).to_string()
}

/// A Cargo/binary-safe name: lowercased, non-alphanumerics collapsed to hyphens.
#[must_use]
pub fn bin_name(short: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true; // leading dashes are dropped
    for ch in short.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() { "component".into() } else { trimmed }
}

/// The dependency declaration a template substitutes for `<<EDGECOMMONS_DEP>>`.
///
/// This is the single source of truth shared by `component new` and `component upgrade`, so
/// the two can never disagree about what a dependency looks like — which is exactly how the
/// Python CLI ended up emitting a Cargo git-tag dependency that its own `upgrade` could not
/// parse (DEF-5).
#[must_use]
pub fn library_dep(language: Language, source: DepSource, library_path: Option<&Path>) -> String {
    match (language, source) {
        (Language::Rust, DepSource::Registry) => {
            format!("git = \"{GIT_URL}\", tag = \"rust-lib/v{EDGECOMMONS_VERSION}\"")
        }
        (Language::Typescript, DepSource::Registry) => format!("^{EDGECOMMONS_VERSION}"),
        // The pinned git requirement a real Python component uses. The Python template used
        // to carry a bare, unpinned `edgecommons` line with a TODO to pin it "once a
        // python-lib release is tagged" — the tags exist, so it is pinned.
        (Language::Python, DepSource::Registry) => format!(
            "edgecommons @ git+{GIT_URL}@python-lib/v{EDGECOMMONS_VERSION}#subdirectory=libs/python"
        ),
        (Language::Rust, DepSource::Local) => format!("path = \"{}\"", posix(library_path)),
        (Language::Typescript, DepSource::Local) => format!("file:{}", posix(library_path)),
        (Language::Python, DepSource::Local) => format!("-e {}", posix(library_path)),
        // Java resolves by version from the published Maven artifact, so its pom substitutes
        // <<EDGECOMMONS_VERSION>> rather than a dependency *fragment*.
        (Language::Java, _) => String::new(),
    }
}

/// The default local library path for a language, relative to a monorepo checkout.
///
/// Only the languages that take a *path* dependency have one; Java resolves by version.
#[must_use]
pub fn default_library_subdir(language: Language) -> Option<&'static str> {
    match language {
        Language::Rust => Some("libs/rust"),
        Language::Typescript => Some("libs/ts"),
        Language::Python => Some("libs/python"),
        Language::Java => None,
    }
}

fn posix(p: Option<&Path>) -> String {
    p.map(|p| p.display().to_string().replace('\\', "/")).unwrap_or_default()
}

/// The placeholder table: the single mapping from token name to value.
#[must_use]
pub fn tokens(language: Language, inputs: &Inputs) -> BTreeMap<String, String> {
    let short = short_name(&inputs.full_name);
    let package = inputs.full_name.to_lowercase();
    let mut t = BTreeMap::new();
    t.insert("COMPONENTFULLNAME".into(), inputs.full_name.clone());
    t.insert("COMPONENTNAME".into(), short.clone());
    t.insert("PACKAGE".into(), package.clone());
    t.insert("PACKAGEPATH".into(), package.replace('.', "/"));
    t.insert("MAINCLASSNAME".into(), format!("{package}.{short}"));
    t.insert("JARNAME".into(), short.clone());
    t.insert("BINNAME".into(), bin_name(&short));
    t.insert("DESCRIPTION".into(), inputs.description.clone());
    t.insert("AUTHOR".into(), inputs.author.clone());
    t.insert("BUCKET".into(), inputs.bucket.clone());
    t.insert("REGION".into(), inputs.region.clone());
    t.insert("EDGECOMMONS_VERSION".into(), EDGECOMMONS_VERSION.into());
    t.insert(
        "EDGECOMMONS_DEP".into(),
        library_dep(language, inputs.dep_source, inputs.library_path.as_deref()),
    );
    t
}

/// The active condition flags a manifest's `conditional` entries test against.
#[must_use]
pub fn flags(inputs: &Inputs, template: &Template) -> Vec<String> {
    let mut f: Vec<String> = inputs.platforms.iter().map(|p| format!("platform:{}", p.as_str())).collect();
    f.push(format!("dep:{}", inputs.dep_source.as_str()));
    f.push(format!("kind:{}", template.manifest.kind.as_str()));
    f
}

/// Generate a component into `target`.
///
/// # Errors
///
/// [`Fatal::Usage`] for bad inputs (a non-empty target without `--force`, a platform the
/// template cannot emit, a required token with no value), [`Fatal::Internal`] for I/O.
pub fn generate(
    template: &Template,
    inputs: &Inputs,
    target: &Path,
    force: bool,
    source_files: Vec<(String, Vec<u8>)>,
) -> Result<Report, Fatal> {
    let mut report = Report::new();

    for p in &inputs.platforms {
        if !template.manifest.supports(*p) {
            return Err(Fatal::Usage(format!(
                "template `{}` cannot emit {} artifacts (it declares: {})",
                template.id(),
                p.as_str(),
                template
                    .manifest
                    .platforms
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }

    if target.is_dir() && target.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
        if !force {
            return Err(Fatal::Usage(format!(
                "target directory `{}` exists and is not empty; pass --force to overwrite",
                target.display()
            )));
        }
        std::fs::remove_dir_all(target)?;
    }

    let values = tokens(template.manifest.language, inputs);
    for req in &template.manifest.requires {
        if values.get(req).is_none_or(String::is_empty) {
            return Err(Fatal::Usage(format!(
                "template `{}` requires a value for <<{req}>> but it is empty",
                template.id()
            )));
        }
    }

    // 1. Prune: platform packs not selected, and unmet conditionals.
    let active = flags(inputs, template);
    let mut pruned: Vec<String> = template.manifest.pruned_packs(&inputs.platforms);
    for c in &template.manifest.conditional {
        if !active.contains(&c.when) {
            pruned.extend(c.paths.iter().cloned());
        }
    }

    let is_pruned = |rel: &str| -> bool {
        pruned.iter().any(|p| {
            let p = p.trim_end_matches('/');
            rel == p || rel.starts_with(&format!("{p}/"))
        })
    };

    // 2. Copy + substitute in one pass.
    for (rel, bytes) in source_files {
        if rel == MANIFEST_NAME || is_pruned(&rel) {
            continue; // the manifest is a template artifact, never shipped to the user
        }
        let subs = template.manifest.substitutions.get(&rel);
        let out_bytes = match (subs, String::from_utf8(bytes.clone())) {
            (Some(token_names), Ok(text)) => {
                let mut text = text;
                for name in token_names {
                    let value = values.get(name).cloned().unwrap_or_default();
                    text = text.replace(&format!("<<{name}>>"), &value);
                }
                text.into_bytes()
            }
            _ => bytes,
        };
        let dest = target.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, out_bytes)?;
    }

    // A manifest that names a file the template does not ship is drift; catch it rather than
    // silently generating a project missing its substitutions.
    for rel in template.manifest.substitutions.keys() {
        if !is_pruned(rel) && !target.join(rel).exists() {
            return Err(Fatal::Internal(format!(
                "manifest for `{}` references `{rel}`, which the template does not ship",
                template.id()
            )));
        }
    }

    // 3. Renames (with {TOKEN} interpolation in the path).
    for r in &template.manifest.renames {
        let from_rel = interpolate(&r.from, &values)?;
        if is_pruned(&from_rel) {
            continue;
        }
        let from = target.join(&from_rel);
        if !from.exists() {
            continue;
        }
        let to = target.join(interpolate(&r.to, &values)?);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&from, &to)?;
    }

    prune_empty_dirs(target);

    // 4. The drift gate: no `<<TOKEN>>` may survive.
    for (path, line, text) in leftover_tokens(target) {
        report.push(
            Diagnostic::error(ec_diag::EC3003_UNSUBSTITUTED_TOKEN, text)
                .with_file(&path)
                .with_line(line)
                .with_help("the template names a token the CLI does not supply (template/CLI drift)"),
        );
    }

    Ok(report)
}

fn interpolate(path: &str, values: &BTreeMap<String, String>) -> Result<String, Fatal> {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            return Err(Fatal::Internal(format!("unterminated placeholder in manifest path `{path}`")));
        };
        let key = &after[..end];
        let Some(v) = values.get(key) else {
            return Err(Fatal::Internal(format!("unknown placeholder `{{{key}}}` in manifest path `{path}`")));
        };
        out.push_str(v);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn prune_empty_dirs(root: &Path) {
    let mut dirs: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_dir() && e.path() != root)
        .map(|e| e.path().to_path_buf())
        .collect();
    // Deepest first, so a directory emptied by its children's removal is itself removed.
    dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for d in dirs {
        if d.read_dir().map(|mut r| r.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&d);
        }
    }
}

/// Find any surviving `<<TOKEN>>` in the generated tree.
fn leftover_tokens(root: &Path) -> Vec<(PathBuf, usize, String)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue; // binary file
        };
        for (i, line) in text.lines().enumerate() {
            if line.contains("<<") && line.contains(">>") {
                out.push((entry.path().to_path_buf(), i + 1, line.trim().to_string()));
            }
        }
    }
    out
}

/// Convenience: generate from an embedded template.
pub fn generate_embedded(
    template: &Template,
    inputs: &Inputs,
    target: &Path,
    force: bool,
) -> Result<Report, Fatal> {
    let files = catalog::files(&template.dir);
    generate(template, inputs, target, force, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;
    use crate::manifest::Kind;

    fn inputs(dep: DepSource, platforms: Vec<Platform>) -> Inputs {
        Inputs {
            full_name: "com.example.MyComponent".into(),
            description: "A test component".into(),
            author: "Test Author".into(),
            platforms,
            dep_source: dep,
            library_path: Some(PathBuf::from("/repo/libs/rust")),
            bucket: "my-bucket".into(),
            region: "us-east-1".into(),
        }
    }

    #[test]
    fn the_library_version_is_resolved_from_the_workspace_not_a_constant() {
        // DEF-1: the Python CLI hardcoded 0.1.0 and emitted a git tag that did not exist.
        // Whatever libs/rust is at, this must equal it — it cannot drift by construction.
        assert!(!EDGECOMMONS_VERSION.is_empty());
        assert!(
            EDGECOMMONS_VERSION.chars().next().is_some_and(|c| c.is_ascii_digit()),
            "expected a semver, got `{EDGECOMMONS_VERSION}`"
        );
        assert_ne!(EDGECOMMONS_VERSION, "0.1.0", "the stale hardcoded version must not reappear");
    }

    #[test]
    fn registry_dep_pins_the_real_current_version() {
        let dep = library_dep(Language::Rust, DepSource::Registry, None);
        assert!(dep.contains(&format!("rust-lib/v{EDGECOMMONS_VERSION}")), "{dep}");
        assert!(!dep.contains("v0.1.0"), "the nonexistent tag must not be emitted: {dep}");
    }

    #[test]
    fn local_dep_uses_a_posix_path_on_every_os() {
        let dep = library_dep(Language::Rust, DepSource::Local, Some(Path::new("C:\\repo\\libs\\rust")));
        assert_eq!(dep, "path = \"C:/repo/libs/rust\"");
        let ts = library_dep(Language::Typescript, DepSource::Local, Some(Path::new("C:\\repo\\libs\\ts")));
        assert_eq!(ts, "file:C:/repo/libs/ts");
    }

    #[test]
    fn bin_names_are_cargo_safe() {
        assert_eq!(bin_name("MyComponent"), "mycomponent");
        assert_eq!(bin_name("My_Cool.Component"), "my-cool-component");
        assert_eq!(bin_name("___"), "component");
    }

    #[test]
    fn short_name_takes_the_last_segment() {
        assert_eq!(short_name("com.example.MyComponent"), "MyComponent");
        assert_eq!(short_name("Bare"), "Bare");
    }

    #[test]
    fn generating_a_host_only_component_emits_no_greengrass_artifacts() {
        // DEF-12: under the Python CLI a HOST-only scaffold still shipped recipe.yaml and
        // gdk-config.json, because only Kubernetes was gated.
        let t = catalog::find(Language::Rust, Kind::Service).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MyComponent");
        let report =
            generate_embedded(&t, &inputs(DepSource::Local, vec![Platform::Host]), &target, false).unwrap();
        assert_eq!(report.error_count(), 0, "{}", report.render_human());

        assert!(!target.join("recipe.yaml").exists(), "a HOST-only scaffold must not carry a GG recipe");
        assert!(!target.join("gdk-config.json").exists());
        assert!(!target.join("k8s").exists());
        // ...and it must carry its own Cargo project.
        assert!(target.join("Cargo.toml").exists());
        assert!(target.join("src/main.rs").exists());
    }

    #[test]
    fn generating_for_greengrass_emits_the_recipe() {
        let t = catalog::find(Language::Rust, Kind::Service).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MyComponent");
        generate_embedded(&t, &inputs(DepSource::Local, vec![Platform::Greengrass]), &target, false).unwrap();
        assert!(target.join("recipe.yaml").exists());
        assert!(target.join("gdk-config.json").exists());
    }

    #[test]
    fn no_unsubstituted_token_survives_any_template() {
        // The drift gate, run across the whole matrix.
        for t in catalog::discover() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("MyComponent");
            let platforms = t.manifest.platforms.clone();
            let lib = match t.manifest.language {
                Language::Rust => Some(PathBuf::from("/repo/libs/rust")),
                Language::Typescript => Some(PathBuf::from("/repo/libs/ts")),
                _ => None,
            };
            let mut i = inputs(DepSource::Local, platforms);
            i.library_path = lib;
            let report = generate_embedded(&t, &i, &target, false)
                .unwrap_or_else(|e| panic!("template {} failed to generate: {e}", t.id()));
            assert_eq!(
                report.error_count(),
                0,
                "template {} left tokens behind:\n{}",
                t.id(),
                report.render_human()
            );
        }
    }

    #[test]
    fn a_non_empty_target_is_refused_without_force() {
        let t = catalog::find(Language::Rust, Kind::Service).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MyComponent");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "important").unwrap();

        let e = generate_embedded(&t, &inputs(DepSource::Local, vec![Platform::Host]), &target, false)
            .unwrap_err();
        assert!(matches!(e, Fatal::Usage(_)), "{e:?}");
        assert!(target.join("keep.txt").exists(), "the refusal must not have deleted anything");

        // --force overwrites.
        generate_embedded(&t, &inputs(DepSource::Local, vec![Platform::Host]), &target, true).unwrap();
        assert!(!target.join("keep.txt").exists());
        assert!(target.join("Cargo.toml").exists());
    }

    #[test]
    fn a_platform_the_template_cannot_emit_is_a_usage_error() {
        let mut t = catalog::find(Language::Rust, Kind::Service).unwrap();
        t.manifest.platforms = vec![Platform::Host];
        let dir = tempfile::tempdir().unwrap();
        let e = generate_embedded(
            &t,
            &inputs(DepSource::Local, vec![Platform::Kubernetes]),
            &dir.path().join("X"),
            false,
        )
        .unwrap_err();
        assert!(matches!(e, Fatal::Usage(_)), "{e:?}");
    }

    #[test]
    fn the_template_manifest_is_never_shipped_to_the_user() {
        let t = catalog::find(Language::Rust, Kind::Service).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MyComponent");
        generate_embedded(&t, &inputs(DepSource::Local, vec![Platform::Host]), &target, false).unwrap();
        assert!(!target.join(MANIFEST_NAME).exists());
    }

    #[test]
    fn java_renames_the_package_path_and_the_class() {
        let t = catalog::find(Language::Java, Kind::Service).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MyComponent");
        let mut i = inputs(DepSource::Local, vec![Platform::Host]);
        i.library_path = None;
        generate_embedded(&t, &i, &target, false).unwrap();

        // com.example.MyComponent -> src/main/java/com/example/mycomponent/MyComponent.java
        let expected = target.join("src/main/java/com/example/mycomponent/MyComponent.java");
        assert!(expected.exists(), "renamed class not found; tree: {:?}", walk(&target));
        // The template's original package directory must not survive the rename.
        assert!(!target.join("src/main/java/com/mbreissi/testcomponent").exists());
    }

    fn walk(root: &Path) -> Vec<String> {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.path().strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn interpolate_rejects_an_unknown_placeholder() {
        let v = BTreeMap::new();
        assert!(interpolate("src/{NOPE}/x", &v).is_err());
    }
}
