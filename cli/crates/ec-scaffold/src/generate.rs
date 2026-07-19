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

/// The edgecommons workspace commit this CLI was built from.
///
/// Resolved **at CLI build time** by `build.rs` (`git rev-parse HEAD`), so a `--dep-source
/// pinned-rev` scaffold pins the library to the exact commit its embedded templates were
/// authored against — the pinned rev by construction contains every facade the template calls.
/// Empty on a non-git build (a source tarball); a `pinned-rev` scaffold without `--library-rev`
/// is then an environment error rather than an emitted `rev = ""`.
pub const EDGECOMMONS_REV: &str = env!("EC_LIBRARY_REV");

const GIT_URL: &str = "https://github.com/edgecommons/edgecommons";

/// Everything `component new` needs.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub full_name: String,
    pub description: String,
    pub author: String,
    pub platforms: Vec<Platform>,
    /// An explicit crate/binary name override (`--bin-name`/`--crate-name`), pre-validated by
    /// the caller. `None` derives the kebab name from the component's short name.
    pub bin_name: Option<String>,
    pub dep_source: DepSource,
    /// Path to a local edgecommons checkout. The **local sibling path** for both
    /// `DepSource::Local` (the path dependency itself) and `DepSource::PinnedRev` (the
    /// `.cargo/config.toml` `[patch]` override that points a pinned-rev build at your working
    /// copy).
    pub library_path: Option<PathBuf>,
    /// The git revision to pin to, for `DepSource::PinnedRev`. Resolved by the caller to
    /// `--library-rev` else [`EDGECOMMONS_REV`]; `None`/empty falls back to `EDGECOMMONS_REV`.
    pub library_rev: Option<String>,
    /// The chosen SPDX license id (e.g. `"BUSL-1.1"`), or `None` for no license
    /// (`--license none`, the default). Sets the `<<LICENSE>>` token and drives the LICENSE-file
    /// writer.
    pub license: Option<&'static str>,
    /// Greengrass-only; prompted and substituted only when the GREENGRASS pack is selected.
    pub bucket: String,
    pub region: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepSource {
    Local,
    Registry,
    /// A git dependency pinned to an exact revision, plus a gitignored local-dev `[patch]`
    /// override. What every shipping sibling component actually uses (Rust/Python only).
    PinnedRev,
}

impl DepSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Registry => "registry",
            Self::PinnedRev => "pinned-rev",
        }
    }
}

/// The short component name: the last dotted segment of `com.example.MyComponent`.
#[must_use]
pub fn short_name(full: &str) -> String {
    full.rsplit('.').next().unwrap_or(full).to_string()
}

/// The kebab-case crate/binary/artifact name derived from the component's short name
/// (DESIGN-cli-scaffold-parity A.1). The **single source** of the `BINNAME` token, so every
/// artifact that names the binary (Cargo.toml, `[[bin]]`, Dockerfile, recipe, supervisor conf,
/// compose service) moves together.
///
/// The algorithm is case-boundary aware and acronym-aware, so `EthernetIpAdapter` becomes
/// `ethernet-ip-adapter` and `OPCUAAdapter` becomes `opcua-adapter` — the ecosystem naming
/// convention (repos and UNS tokens are kebab), which the previous collapse-non-alphanumerics
/// rule violated (`ethernetipadapter`).
///
/// Classify each char as U(pper), L(ower), D(igit), or S(eparator, dropped). A word boundary
/// sits between adjacent kept chars `c1,c2` when either:
///   (a) `class(c1) ∈ {L,D}` and `class(c2) = U` — a lower/digit → Upper transition; or
///   (b) `class(c1) = U`, `class(c2) = U`, and the char **after** `c2` is `L` — the end of an
///       uppercase acronym run (`HTTPServer` → `http`|`server`).
/// Separators are boundaries in their own right and are dropped. Words are lowercased and joined
/// with `-`; an empty result becomes `"component"`.
#[must_use]
pub fn bin_name(short: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Class {
        U,
        L,
        D,
        S,
    }
    fn classify(c: char) -> Class {
        if c.is_ascii_uppercase() {
            Class::U
        } else if c.is_ascii_lowercase() {
            Class::L
        } else if c.is_ascii_digit() {
            Class::D
        } else {
            Class::S
        }
    }

    let chars: Vec<char> = short.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut prev: Option<Class> = None;

    for (i, &c) in chars.iter().enumerate() {
        let cls = classify(c);
        if cls == Class::S {
            // A separator ends the current word and is itself dropped.
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            prev = None;
            continue;
        }
        // A boundary can only exist *between* two kept chars, so only when a word is in progress.
        if let Some(p) = prev {
            let next = chars.get(i + 1).map(|&n| classify(n));
            let boundary = match (p, cls) {
                (Class::L | Class::D, Class::U) => true,
                (Class::U, Class::U) => next == Some(Class::L),
                _ => false,
            };
            if boundary {
                words.push(std::mem::take(&mut word));
            }
        }
        word.push(c.to_ascii_lowercase());
        prev = Some(cls);
    }
    if !word.is_empty() {
        words.push(word);
    }

    let joined = words.join("-");
    if joined.is_empty() {
        "component".into()
    } else {
        joined
    }
}

/// The dependency declaration a template substitutes for `<<EDGECOMMONS_DEP>>`.
///
/// This is the single source of truth shared by `component new` and `component upgrade`, so
/// the two can never disagree about what a dependency looks like — which is exactly how the
/// Python CLI ended up emitting a Cargo git-tag dependency that its own `upgrade` could not
/// parse (DEF-5).
#[must_use]
pub fn library_dep(
    language: Language,
    source: DepSource,
    library_path: Option<&Path>,
    rev: Option<&str>,
) -> String {
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
        // pinned-rev: the exact workspace commit this CLI was built from (or `--library-rev`).
        // Rust/Python only — Maven and npm cannot express a git dep on a monorepo subdirectory,
        // so those are rejected in `component.rs` before generation ever calls this.
        (Language::Rust, DepSource::PinnedRev) => {
            format!("git = \"{GIT_URL}\", rev = \"{}\"", resolve_rev(rev))
        }
        (Language::Python, DepSource::PinnedRev) => {
            format!(
                "edgecommons @ git+{GIT_URL}@{}#subdirectory=libs/python",
                resolve_rev(rev)
            )
        }
        (Language::Rust, DepSource::Local) => format!("path = \"{}\"", posix(library_path)),
        (Language::Typescript, DepSource::Local) => format!("file:{}", posix(library_path)),
        (Language::Python, DepSource::Local) => format!("-e {}", posix(library_path)),
        // Java resolves by version from the published Maven artifact, so its pom substitutes
        // <<EDGECOMMONS_VERSION>> rather than a dependency *fragment*. Java/TypeScript pinned-rev
        // is rejected upstream, so this arm is never reached for it in practice.
        (Language::Java | Language::Typescript, DepSource::PinnedRev) | (Language::Java, _) => {
            String::new()
        }
    }
}

/// The rev to pin to: the explicit `--library-rev` when non-empty, else the CLI's build rev.
fn resolve_rev(rev: Option<&str>) -> &str {
    match rev {
        Some(r) if !r.is_empty() => r,
        _ => EDGECOMMONS_REV,
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
    p.map(|p| p.display().to_string().replace('\\', "/"))
        .unwrap_or_default()
}

/// The placeholder table: the single mapping from token name to value.
#[must_use]
pub fn tokens(language: Language, inputs: &Inputs) -> BTreeMap<String, String> {
    let short = short_name(&inputs.full_name);
    let package = inputs.full_name.to_lowercase();
    // BINNAME is the single kebab crate/bin/artifact name; `--bin-name` overrides the derivation.
    let binname = inputs.bin_name.clone().unwrap_or_else(|| bin_name(&short));
    let mut t = BTreeMap::new();
    t.insert("COMPONENTFULLNAME".into(), inputs.full_name.clone());
    t.insert("COMPONENTNAME".into(), short.clone());
    t.insert("PACKAGE".into(), package.clone());
    t.insert("PACKAGEPATH".into(), package.replace('.', "/"));
    t.insert("MAINCLASSNAME".into(), format!("{package}.{short}"));
    // JARNAME follows the Maven artifactId/finalName convention (lower-kebab) — it is BINNAME,
    // not the PascalCase short name. The Java *class* stays PascalCase (MAINCLASSNAME) and the
    // Greengrass component name stays reverse-DNS (COMPONENTFULLNAME).
    t.insert("JARNAME".into(), binname.clone());
    t.insert("BINNAME".into(), binname.clone());
    // The Python module-dir name: BINNAME with `-` → `_` (`ethernet-ip-adapter` →
    // `ethernet_ip_adapter`), the `modbus_adapter`-style importable package name.
    t.insert("SNAKENAME".into(), binname.replace('-', "_"));
    t.insert("DESCRIPTION".into(), inputs.description.clone());
    t.insert("AUTHOR".into(), inputs.author.clone());
    t.insert("BUCKET".into(), inputs.bucket.clone());
    t.insert("REGION".into(), inputs.region.clone());
    // The local sibling path the pinned-rev `.cargo/config.toml` `[patch]` override points at.
    t.insert(
        "LIBRARY_LOCAL_PATH".into(),
        posix(inputs.library_path.as_deref()),
    );
    // The chosen SPDX id (or empty for `--license none`). Consumed by manifest license fields in
    // a later template phase; the LICENSE *file* is written by `component.rs`.
    t.insert("LICENSE".into(), inputs.license.unwrap_or("").into());
    t.insert("EDGECOMMONS_VERSION".into(), EDGECOMMONS_VERSION.into());
    t.insert(
        "EDGECOMMONS_DEP".into(),
        library_dep(
            language,
            inputs.dep_source,
            inputs.library_path.as_deref(),
            inputs.library_rev.as_deref(),
        ),
    );
    t
}

/// The active condition flags a manifest's `conditional` entries test against.
#[must_use]
pub fn flags(inputs: &Inputs, template: &Template) -> Vec<String> {
    let mut f: Vec<String> = inputs
        .platforms
        .iter()
        .map(|p| format!("platform:{}", p.as_str()))
        .collect();
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

    if target.is_dir()
        && target
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    {
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
                .with_help(
                    "the template names a token the CLI does not supply (template/CLI drift)",
                ),
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
            return Err(Fatal::Internal(format!(
                "unterminated placeholder in manifest path `{path}`"
            )));
        };
        let key = &after[..end];
        let Some(v) = values.get(key) else {
            return Err(Fatal::Internal(format!(
                "unknown placeholder `{{{key}}}` in manifest path `{path}`"
            )));
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
        if d.read_dir()
            .map(|mut r| r.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&d);
        }
    }
}

/// Find any surviving `<<TOKEN>>` in the generated tree.
fn leftover_tokens(root: &Path) -> Vec<(PathBuf, usize, String)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
    {
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
            bin_name: None,
            dep_source: dep,
            library_path: Some(PathBuf::from("/repo/libs/rust")),
            library_rev: None,
            license: None,
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
            EDGECOMMONS_VERSION
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()),
            "expected a semver, got `{EDGECOMMONS_VERSION}`"
        );
        assert_ne!(
            EDGECOMMONS_VERSION, "0.1.0",
            "the stale hardcoded version must not reappear"
        );
    }

    #[test]
    fn registry_dep_pins_the_real_current_version() {
        let dep = library_dep(Language::Rust, DepSource::Registry, None, None);
        assert!(
            dep.contains(&format!("rust-lib/v{EDGECOMMONS_VERSION}")),
            "{dep}"
        );
        assert!(
            !dep.contains("v0.1.0"),
            "the nonexistent tag must not be emitted: {dep}"
        );
    }

    #[test]
    fn local_dep_uses_a_posix_path_on_every_os() {
        let dep = library_dep(
            Language::Rust,
            DepSource::Local,
            Some(Path::new("C:\\repo\\libs\\rust")),
            None,
        );
        assert_eq!(dep, "path = \"C:/repo/libs/rust\"");
        let ts = library_dep(
            Language::Typescript,
            DepSource::Local,
            Some(Path::new("C:\\repo\\libs\\ts")),
            None,
        );
        assert_eq!(ts, "file:C:/repo/libs/ts");
    }

    #[test]
    fn pinned_rev_dep_pins_the_build_rev() {
        // With no explicit rev, the Rust and Python pins carry the CLI's build rev.
        let rust = library_dep(Language::Rust, DepSource::PinnedRev, None, None);
        assert_eq!(
            rust,
            format!("git = \"{GIT_URL}\", rev = \"{EDGECOMMONS_REV}\"")
        );
        let py = library_dep(Language::Python, DepSource::PinnedRev, None, None);
        assert_eq!(
            py,
            format!("edgecommons @ git+{GIT_URL}@{EDGECOMMONS_REV}#subdirectory=libs/python")
        );
        // An explicit rev wins over the build rev.
        let pinned = library_dep(Language::Rust, DepSource::PinnedRev, None, Some("deadbeef"));
        assert_eq!(pinned, format!("git = \"{GIT_URL}\", rev = \"deadbeef\""));
    }

    #[test]
    fn bin_names_are_cargo_safe() {
        // The A.1 example table — case-boundary and acronym aware.
        assert_eq!(bin_name("MyComponent"), "my-component");
        assert_eq!(bin_name("EthernetIpAdapter"), "ethernet-ip-adapter");
        assert_eq!(bin_name("OPCUAAdapter"), "opcua-adapter");
        assert_eq!(bin_name("ModbusTCPAdapter"), "modbus-tcp-adapter");
        assert_eq!(bin_name("Modbus2Tcp"), "modbus2-tcp");
        assert_eq!(bin_name("My_Cool.Component"), "my-cool-component");
        assert_eq!(bin_name("mycomponent"), "mycomponent");
        assert_eq!(bin_name("___"), "component");
        assert_eq!(bin_name("HTTPServer"), "http-server");
    }

    #[test]
    fn snakename_is_binname_with_underscores() {
        let mut i = inputs(DepSource::Local, vec![Platform::Host]);
        i.full_name = "com.example.EthernetIpAdapter".into();
        let t = tokens(Language::Python, &i);
        assert_eq!(t["BINNAME"], "ethernet-ip-adapter");
        assert_eq!(t["SNAKENAME"], "ethernet_ip_adapter");
    }

    #[test]
    fn a_bin_name_override_drives_binname_snakename_and_jarname() {
        let mut i = inputs(DepSource::Local, vec![Platform::Host]);
        i.bin_name = Some("my-override".into());
        let t = tokens(Language::Rust, &i);
        assert_eq!(t["BINNAME"], "my-override");
        assert_eq!(t["JARNAME"], "my-override");
        assert_eq!(t["SNAKENAME"], "my_override");
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
        let report = generate_embedded(
            &t,
            &inputs(DepSource::Local, vec![Platform::Host]),
            &target,
            false,
        )
        .unwrap();
        assert_eq!(report.error_count(), 0, "{}", report.render_human());

        assert!(
            !target.join("recipe.yaml").exists(),
            "a HOST-only scaffold must not carry a GG recipe"
        );
        assert!(!target.join("gdk-config.json").exists());
        assert!(!target.join("k8s").exists());
        // ...and it must carry its own Cargo project.
        assert!(target.join("Cargo.toml").exists());
        assert!(target.join("src/main.rs").exists());
    }

    #[test]
    fn the_cargo_override_is_emitted_only_under_pinned_rev() {
        let t = catalog::find(Language::Rust, Kind::ProtocolAdapter).unwrap();

        // pinned-rev: the .cargo/config.toml patch override is emitted, tokens substituted.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out");
        let mut i = inputs(DepSource::PinnedRev, vec![Platform::Host]);
        i.library_rev = Some("abc1234".into());
        let report = generate_embedded(&t, &i, &target, false).unwrap();
        assert_eq!(report.error_count(), 0, "{}", report.render_human());
        let cargo_cfg = target.join(".cargo/config.toml");
        assert!(
            cargo_cfg.exists(),
            "pinned-rev must emit .cargo/config.toml"
        );
        let text = std::fs::read_to_string(&cargo_cfg).unwrap();
        assert!(text.contains("[patch."), "{text}");
        assert!(
            text.contains("https://github.com/edgecommons/edgecommons"),
            "the patch must target the git source: {text}"
        );
        assert!(!text.contains("<<"), "tokens must be substituted: {text}");

        // local and registry: no .cargo dir (the conditional prunes it).
        for dep in [DepSource::Local, DepSource::Registry] {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("out");
            generate_embedded(&t, &inputs(dep, vec![Platform::Host]), &target, false).unwrap();
            assert!(
                !target.join(".cargo").exists(),
                "{:?} must not emit the .cargo override",
                dep.as_str()
            );
        }
    }

    #[test]
    fn generating_for_greengrass_emits_the_recipe() {
        let t = catalog::find(Language::Rust, Kind::Service).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("MyComponent");
        generate_embedded(
            &t,
            &inputs(DepSource::Local, vec![Platform::Greengrass]),
            &target,
            false,
        )
        .unwrap();
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

        let e = generate_embedded(
            &t,
            &inputs(DepSource::Local, vec![Platform::Host]),
            &target,
            false,
        )
        .unwrap_err();
        assert!(matches!(e, Fatal::Usage(_)), "{e:?}");
        assert!(
            target.join("keep.txt").exists(),
            "the refusal must not have deleted anything"
        );

        // --force overwrites.
        generate_embedded(
            &t,
            &inputs(DepSource::Local, vec![Platform::Host]),
            &target,
            true,
        )
        .unwrap();
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
        generate_embedded(
            &t,
            &inputs(DepSource::Local, vec![Platform::Host]),
            &target,
            false,
        )
        .unwrap();
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
        assert!(
            expected.exists(),
            "renamed class not found; tree: {:?}",
            walk(&target)
        );
        // The template's original package directory must not survive the rename.
        assert!(
            !target
                .join("src/main/java/com/mbreissi/testcomponent")
                .exists()
        );
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
