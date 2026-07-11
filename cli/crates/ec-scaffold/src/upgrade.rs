//! `component upgrade` and `component version` (DESIGN-cli §7).
//!
//! The Python `upgrade` was a set of regexes that were wrong for three of the four languages:
//!
//! * **TypeScript** — it matched the bare key `"edgecommons"`, but the template emits the
//!   scoped key `"@edgecommons/edgecommons"`, so it was a **silent no-op for every generated
//!   TS component** (DEF-3).
//! * **Python** — it rewrote any line starting with `edgecommons` to `edgecommons==X`, which
//!   **destroys** the `edgecommons @ git+https://…#subdirectory=libs/python` form real
//!   components use (DEF-4).
//! * **Rust** — it matched only `version = "…"` forms, so it could not bump the **git-tag
//!   dependency the CLI itself emits** for `--dep-source registry` (DEF-5).
//!
//! Two commands disagreeing about what a dependency looks like is a structural problem, not a
//! regex problem. Here, `component new` and `component upgrade` share one dependency table
//! ([`crate::generate::library_dep`]), so they cannot drift apart again.

use std::path::Path;

use ec_diag::{Diagnostic, Fatal, Report};

/// What a bump did to one manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The dependency moved to a new version.
    Bumped {
        file: String,
        from: String,
        to: String,
    },
    /// A path/editable dependency: correct to leave alone, and said so.
    PathDependency { file: String },
    /// The manifest exists but declares no edgecommons dependency.
    NotFound { file: String },
}

impl Change {
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Bumped { file, from, to } => format!("{file}: edgecommons {from} -> {to}"),
            Self::PathDependency { file } => {
                format!("{file}: edgecommons is a path dependency; nothing to version-bump")
            }
            Self::NotFound { file } => format!("{file}: no edgecommons dependency found"),
        }
    }
}

/// Bump a component's **edgecommons library** dependency across whichever manifests it ships.
///
/// # Errors
///
/// [`Fatal::Usage`] if the project directory does not exist; [`Fatal::Internal`] on I/O.
pub fn upgrade(root: &Path, to: &str, dry_run: bool) -> Result<(Vec<Change>, Report), Fatal> {
    if !root.is_dir() {
        return Err(Fatal::Usage(format!(
            "no such component directory: {}",
            root.display()
        )));
    }
    let mut changes = Vec::new();
    let mut report = Report::new();

    if let Some(c) = bump_cargo(&root.join("Cargo.toml"), to, dry_run)? {
        changes.push(c);
    }
    if let Some(c) = bump_package_json(&root.join("package.json"), to, dry_run)? {
        changes.push(c);
    }
    if let Some(c) = bump_requirements(&root.join("requirements.txt"), to, dry_run)? {
        changes.push(c);
    }
    if let Some(c) = bump_pom(&root.join("pom.xml"), to, dry_run)? {
        changes.push(c);
    }

    if changes.is_empty() {
        report.push(
            Diagnostic::warning(
                ec_diag::Code("EC4004"),
                "no dependency manifest found (Cargo.toml, package.json, requirements.txt, pom.xml)"
                    .to_string(),
            )
            .with_file(root),
        );
    }
    Ok((changes, report))
}

/// Rust — TOML-parsed, and **the git-tag form is supported**, which is what the CLI emits for
/// `--dep-source registry`.
fn bump_cargo(path: &Path, to: &str, dry_run: bool) -> Result<Option<Change>, Fatal> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let file = "Cargo.toml".to_string();

    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| Fatal::Internal(format!("Cargo.toml is not valid TOML: {e}")))?;

    let Some(dep) = doc
        .get_mut("dependencies")
        .and_then(|d| d.get_mut("edgecommons"))
    else {
        return Ok(Some(Change::NotFound { file }));
    };

    // `edgecommons = "0.2.0"` — a plain version.
    if let Some(v) = dep.as_str() {
        let from = v.to_string();
        if !dry_run {
            if let Some(val) = dep.as_value_mut() {
                set_preserving_decor(val, to);
            }
            write(path, &doc.to_string())?;
        }
        return Ok(Some(Change::Bumped {
            file,
            from,
            to: to.into(),
        }));
    }

    // An inline table: `{ path = … }`, `{ version = … }`, or `{ git = …, tag = … }`.
    if let Some(t) = dep.as_inline_table_mut() {
        if t.contains_key("path") {
            return Ok(Some(Change::PathDependency { file }));
        }
        // The git-tag form the CLI emits: bump the tag, not a version key.
        if t.contains_key("git")
            && let Some(tag) = t.get_mut("tag")
        {
            let from = tag.as_str().unwrap_or_default().to_string();
            let new_tag = format!("rust-lib/v{to}");
            if !dry_run {
                set_preserving_decor(tag, &new_tag);
                write(path, &doc.to_string())?;
            }
            return Ok(Some(Change::Bumped {
                file,
                from,
                to: new_tag,
            }));
        }
        if let Some(v) = t.get_mut("version") {
            let from = v.as_str().unwrap_or_default().to_string();
            if !dry_run {
                set_preserving_decor(v, to);
                write(path, &doc.to_string())?;
            }
            return Ok(Some(Change::Bumped {
                file,
                from,
                to: to.into(),
            }));
        }
    }

    Ok(Some(Change::NotFound { file }))
}

/// TypeScript — the **scoped** key, which the Python CLI never matched (DEF-3).
fn bump_package_json(path: &Path, to: &str, dry_run: bool) -> Result<Option<Change>, Fatal> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let file = "package.json".to_string();

    let mut doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Fatal::Internal(format!("package.json is not valid JSON: {e}")))?;

    const KEYS: [&str; 2] = ["@edgecommons/edgecommons", "edgecommons"];

    for section in ["dependencies", "devDependencies"] {
        let Some(deps) = doc
            .get_mut(section)
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        for key in KEYS {
            let Some(current) = deps.get(key).and_then(serde_json::Value::as_str) else {
                continue;
            };
            // `file:`/`link:` are the local-dev forms; leave them exactly as they are.
            if current.starts_with("file:") || current.starts_with("link:") {
                return Ok(Some(Change::PathDependency { file }));
            }
            let from = current.to_string();
            let new = format!("^{to}");
            if !dry_run {
                deps.insert(key.to_string(), serde_json::Value::String(new.clone()));
                let mut out = serde_json::to_string_pretty(&doc)
                    .map_err(|e| Fatal::Internal(e.to_string()))?;
                out.push('\n');
                write(path, &out)?;
            }
            return Ok(Some(Change::Bumped {
                file,
                from,
                to: new,
            }));
        }
    }
    Ok(Some(Change::NotFound { file }))
}

/// Python — **form-preserving**. The Python CLI flattened every form to `edgecommons==X`,
/// destroying the git pin real components use (DEF-4).
fn bump_requirements(path: &Path, to: &str, dry_run: bool) -> Result<Option<Change>, Fatal> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let file = "requirements.txt".to_string();

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }

        // An editable sibling install: the local-dev form. Leave it alone.
        if trimmed.starts_with("-e ") && trimmed.contains("libs/python") {
            return Ok(Some(Change::PathDependency { file }));
        }

        // The pinned git requirement: rewrite only the tag, preserving the whole form.
        if trimmed.starts_with("edgecommons @ git+") {
            let from = trimmed.to_string();
            let Some(at) = trimmed.rfind("@python-lib/v") else {
                continue;
            };
            let Some(frag) = trimmed[at..].find('#') else {
                continue;
            };
            let new = format!(
                "{}@python-lib/v{}{}",
                &trimmed[..at],
                to,
                &trimmed[at + frag..]
            );
            if !dry_run {
                *line = new.clone();
                write(path, &(lines_to_text(&lines)))?;
            }
            return Ok(Some(Change::Bumped {
                file,
                from,
                to: new,
            }));
        }

        // A plain pin or a bare requirement.
        if trimmed == "edgecommons" || trimmed.starts_with("edgecommons==") {
            let from = trimmed.to_string();
            let new = format!("edgecommons=={to}");
            if !dry_run {
                *line = new.clone();
                write(path, &(lines_to_text(&lines)))?;
            }
            return Ok(Some(Change::Bumped {
                file,
                from,
                to: new,
            }));
        }
    }
    Ok(Some(Change::NotFound { file }))
}

fn lines_to_text(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// Java — the pom's edgecommons `<version>`, located structurally rather than by a regex that
/// demands `<artifactId>` be immediately followed by `<version>`.
fn bump_pom(path: &Path, to: &str, dry_run: bool) -> Result<Option<Change>, Fatal> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let file = "pom.xml".to_string();

    // Find the <dependency> block declaring artifactId edgecommons, then the <version> inside
    // it. Splicing by byte range preserves the pom's formatting exactly, which a full
    // parse-and-serialize would not.
    let Some((from, span)) = find_dependency_version(&text, "edgecommons") else {
        return Ok(Some(Change::NotFound { file }));
    };

    // A property reference (`${edgecommons.version}`) is indirection, not a version: bump the
    // property instead, and if that is absent say so rather than corrupting the pom.
    if from.starts_with("${") {
        let prop = from.trim_start_matches("${").trim_end_matches('}');
        let Some((pfrom, pspan)) = find_property(&text, prop) else {
            return Ok(Some(Change::NotFound { file }));
        };
        if !dry_run {
            let mut out = text.clone();
            out.replace_range(pspan, to);
            write(path, &out)?;
        }
        return Ok(Some(Change::Bumped {
            file,
            from: pfrom,
            to: to.into(),
        }));
    }

    if !dry_run {
        let mut out = text.clone();
        out.replace_range(span, to);
        write(path, &out)?;
    }
    Ok(Some(Change::Bumped {
        file,
        from,
        to: to.into(),
    }))
}

/// The text and byte range of the `<version>` inside the `<dependency>` whose `<artifactId>`
/// is `artifact`.
fn find_dependency_version(xml: &str, artifact: &str) -> Option<(String, std::ops::Range<usize>)> {
    let needle = format!("<artifactId>{artifact}</artifactId>");
    let mut search_from = 0usize;
    while let Some(rel) = xml[search_from..].find(&needle) {
        let at = search_from + rel;
        // Bound the search to this <dependency> element, so we cannot pick up a sibling's
        // <version> when this one has none.
        let block_start = xml[..at].rfind("<dependency>").unwrap_or(0);
        let block_end = xml[at..]
            .find("</dependency>")
            .map_or(xml.len(), |e| at + e);
        if let Some(span) = find_tag(&xml[block_start..block_end], "version") {
            let abs = (block_start + span.start)..(block_start + span.end);
            return Some((xml[abs.clone()].to_string(), abs));
        }
        search_from = at + needle.len();
    }
    None
}

fn find_property(xml: &str, name: &str) -> Option<(String, std::ops::Range<usize>)> {
    let span = find_tag(xml, name)?;
    Some((xml[span.clone()].to_string(), span))
}

/// The byte range of the text inside the first `<tag>…</tag>`.
fn find_tag(xml: &str, tag: &str) -> Option<std::ops::Range<usize>> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(s..e)
}

fn write(path: &Path, text: &str) -> Result<(), Fatal> {
    std::fs::write(path, text).map_err(|e| Fatal::Internal(format!("{}: {e}", path.display())))
}

/// Replace a TOML value **without destroying its decor** — the surrounding whitespace and any
/// trailing comment.
///
/// `toml_edit::value()` swaps the whole item and takes the decor with it, so a naive bump eats
/// the author's `# keep this pinned until X` comment. A tool that silently deletes comments is
/// a tool people stop running.
fn set_preserving_decor(v: &mut toml_edit::Value, new: &str) {
    let decor = v.decor().clone();
    *v = toml_edit::Value::from(new);
    *v.decor_mut() = decor;
}

/// Set the **component's own** version — a different thing from [`upgrade`], which moves the
/// edgecommons library dependency. Conflating the two is a trap the Python CLI avoided only by
/// not having the second one.
pub fn set_component_version(root: &Path, to: &str, dry_run: bool) -> Result<Vec<Change>, Fatal> {
    if !root.is_dir() {
        return Err(Fatal::Usage(format!(
            "no such component directory: {}",
            root.display()
        )));
    }
    if !is_semver(to) {
        return Err(Fatal::Usage(format!(
            "`{to}` is not a version (expected e.g. 0.3.0)"
        )));
    }

    let mut changes = Vec::new();

    // Cargo.toml: [package] version
    let cargo = root.join("Cargo.toml");
    if let Ok(text) = std::fs::read_to_string(&cargo) {
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e| Fatal::Internal(format!("Cargo.toml: {e}")))?;
        if let Some(v) = doc.get_mut("package").and_then(|p| p.get_mut("version")) {
            let from = v.as_str().unwrap_or_default().to_string();
            if !dry_run {
                if let Some(val) = v.as_value_mut() {
                    set_preserving_decor(val, to);
                }
                write(&cargo, &doc.to_string())?;
            }
            changes.push(Change::Bumped {
                file: "Cargo.toml".into(),
                from,
                to: to.into(),
            });
        }
    }

    // package.json: version
    let pkg = root.join("package.json");
    if let Ok(text) = std::fs::read_to_string(&pkg) {
        let mut doc: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| Fatal::Internal(format!("package.json: {e}")))?;
        if let Some(v) = doc.get_mut("version") {
            let from = v.as_str().unwrap_or_default().to_string();
            if !dry_run {
                *v = serde_json::Value::String(to.into());
                let mut out = serde_json::to_string_pretty(&doc)
                    .map_err(|e| Fatal::Internal(e.to_string()))?;
                out.push('\n');
                write(&pkg, &out)?;
            }
            changes.push(Change::Bumped {
                file: "package.json".into(),
                from,
                to: to.into(),
            });
        }
    }

    // pom.xml: the project's own <version>, which is the FIRST <version> outside any
    // <dependency> — not the edgecommons dependency's.
    let pom = root.join("pom.xml");
    if let Ok(text) = std::fs::read_to_string(&pom)
        && let Some((from, span)) = find_project_version(&text)
    {
        if !dry_run {
            let mut out = text.clone();
            out.replace_range(span, to);
            write(&pom, &out)?;
        }
        changes.push(Change::Bumped {
            file: "pom.xml".into(),
            from,
            to: to.into(),
        });
    }

    // gdk-config.json: the Greengrass component version. Without this, a scaffold keeps its
    // `NEXT_PATCH` and `component release` refuses it — which is correct, but leaves the author
    // with no way forward. This is the way forward.
    let gdk = root.join("gdk-config.json");
    if let Ok(text) = std::fs::read_to_string(&gdk) {
        let mut doc: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| Fatal::Internal(format!("gdk-config.json: {e}")))?;
        if let Some(component) = doc
            .get_mut("component")
            .and_then(serde_json::Value::as_object_mut)
            && let Some((_, body)) = component.iter_mut().next()
            && let Some(v) = body.get_mut("version")
        {
            let from = v.as_str().unwrap_or_default().to_string();
            if !dry_run {
                *v = serde_json::Value::String(to.into());
                let mut out = serde_json::to_string_pretty(&doc)
                    .map_err(|e| Fatal::Internal(e.to_string()))?;
                out.push('\n');
                write(&gdk, &out)?;
            }
            changes.push(Change::Bumped {
                file: "gdk-config.json".into(),
                from,
                to: to.into(),
            });
        }
    }

    Ok(changes)
}

/// The project's own `<version>` in a pom: the first one that is not inside a `<dependency>`.
fn find_project_version(xml: &str) -> Option<(String, std::ops::Range<usize>)> {
    let mut from = 0usize;
    loop {
        let rel = xml[from..].find("<version>")?;
        let at = from + rel;
        // Is this version inside a <dependency> block?
        let dep_open = xml[..at].rfind("<dependency>");
        let dep_close = xml[..at].rfind("</dependency>");
        let inside_dependency = match (dep_open, dep_close) {
            (Some(o), Some(c)) => o > c,
            (Some(_), None) => true,
            _ => false,
        };
        if !inside_dependency {
            let s = at + "<version>".len();
            let e = xml[s..].find("</version>")? + s;
            return Some((xml[s..e].to_string(), s..e));
        }
        from = at + "<version>".len();
    }
}

#[must_use]
pub fn is_semver(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// The default library version this CLI scaffolds against — used for `--to` guidance.
#[must_use]
pub fn current_library_version() -> &'static str {
    crate::generate::EDGECOMMONS_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let p = d.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        d
    }

    #[test]
    fn typescript_scoped_key_is_bumped() {
        // DEF-3: the Python CLI matched the bare key, so this was a silent no-op for EVERY
        // generated TypeScript component.
        let d = project(&[(
            "package.json",
            r#"{"name":"x","version":"1.0.0","dependencies":{"@edgecommons/edgecommons":"^0.2.0"}}"#,
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(
            matches!(&changes[0], Change::Bumped { to, .. } if to == "^0.3.0"),
            "{changes:?}"
        );

        let text = std::fs::read_to_string(d.path().join("package.json")).unwrap();
        assert!(
            text.contains(r#""@edgecommons/edgecommons": "^0.3.0""#),
            "{text}"
        );
    }

    #[test]
    fn typescript_file_dependency_is_left_alone() {
        let d = project(&[(
            "package.json",
            r#"{"dependencies":{"@edgecommons/edgecommons":"file:../libs/ts"}}"#,
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(
            matches!(&changes[0], Change::PathDependency { .. }),
            "{changes:?}"
        );
        let text = std::fs::read_to_string(d.path().join("package.json")).unwrap();
        assert!(
            text.contains("file:../libs/ts"),
            "a path dep must not be rewritten"
        );
    }

    #[test]
    fn python_git_pin_is_preserved_not_destroyed() {
        // DEF-4: the Python CLI rewrote this whole line to `edgecommons==0.3.0`, throwing away
        // the git URL and the subdirectory — the component then could not install at all.
        let d = project(&[(
            "requirements.txt",
            "psutil>=5.9.6\nedgecommons @ git+https://github.com/edgecommons/edgecommons@python-lib/v0.2.0#subdirectory=libs/python\n",
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        let text = std::fs::read_to_string(d.path().join("requirements.txt")).unwrap();
        assert!(
            text.contains("git+https://github.com/edgecommons/edgecommons"),
            "{text}"
        );
        assert!(text.contains("@python-lib/v0.3.0"), "{text}");
        assert!(
            text.contains("#subdirectory=libs/python"),
            "the fragment must survive: {text}"
        );
        assert!(
            text.contains("psutil>=5.9.6"),
            "other requirements must survive: {text}"
        );
        assert!(matches!(&changes[0], Change::Bumped { .. }));
    }

    #[test]
    fn python_editable_sibling_install_is_left_alone() {
        let d = project(&[("requirements.txt", "psutil>=5.9.6\n-e /repo/libs/python\n")]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(
            matches!(&changes[0], Change::PathDependency { .. }),
            "{changes:?}"
        );
    }

    #[test]
    fn python_plain_pin_is_bumped() {
        let d = project(&[("requirements.txt", "edgecommons==0.2.0\n")]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(matches!(&changes[0], Change::Bumped { .. }));
        let text = std::fs::read_to_string(d.path().join("requirements.txt")).unwrap();
        assert!(text.contains("edgecommons==0.3.0"));
    }

    #[test]
    fn rust_git_tag_dependency_is_bumped() {
        // DEF-5: `component new --dep-source registry` emits exactly this, and the Python
        // `upgrade` could not parse it — the two commands disagreed about what a dependency is.
        let d = project(&[(
            "Cargo.toml",
            "[package]\nname = \"x\"\nversion = \"1.0.0\"\n\n[dependencies]\nedgecommons = { git = \"https://github.com/edgecommons/edgecommons\", tag = \"rust-lib/v0.2.0\", default-features = false }\n",
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(matches!(&changes[0], Change::Bumped { .. }), "{changes:?}");
        let text = std::fs::read_to_string(d.path().join("Cargo.toml")).unwrap();
        assert!(text.contains(r#"tag = "rust-lib/v0.3.0""#), "{text}");
        // The rest of the dependency must survive untouched.
        assert!(text.contains("default-features = false"), "{text}");
    }

    #[test]
    fn rust_path_dependency_is_left_alone() {
        let d = project(&[(
            "Cargo.toml",
            "[package]\nname=\"x\"\nversion=\"1.0.0\"\n\n[dependencies]\nedgecommons = { path = \"/repo/libs/rust\" }\n",
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(
            matches!(&changes[0], Change::PathDependency { .. }),
            "{changes:?}"
        );
    }

    #[test]
    fn rust_plain_version_dependency_is_bumped() {
        let d = project(&[(
            "Cargo.toml",
            "[package]\nname=\"x\"\nversion=\"1.0.0\"\n\n[dependencies]\nedgecommons = \"0.2.0\"\n",
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(matches!(&changes[0], Change::Bumped { .. }));
        let text = std::fs::read_to_string(d.path().join("Cargo.toml")).unwrap();
        assert!(text.contains(r#"edgecommons = "0.3.0""#), "{text}");
    }

    #[test]
    fn java_pom_version_is_bumped_and_formatting_preserved() {
        let d = project(&[(
            "pom.xml",
            r#"<project>
  <dependencies>
    <dependency>
      <groupId>com.mbreissi.edgecommons</groupId>
      <artifactId>edgecommons</artifactId>
      <version>0.2.0</version>
    </dependency>
  </dependencies>
</project>
"#,
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(matches!(&changes[0], Change::Bumped { .. }), "{changes:?}");
        let text = std::fs::read_to_string(d.path().join("pom.xml")).unwrap();
        assert!(text.contains("<version>0.3.0</version>"), "{text}");
        assert!(
            text.contains("<groupId>com.mbreissi.edgecommons</groupId>"),
            "formatting must survive"
        );
    }

    #[test]
    fn java_pom_with_a_property_indirection_bumps_the_property() {
        // The Python regex required <artifactId> to be immediately followed by <version>, so
        // it silently failed on a property-based pom.
        let d = project(&[(
            "pom.xml",
            r#"<project>
  <properties>
    <edgecommons.version>0.2.0</edgecommons.version>
  </properties>
  <dependencies>
    <dependency>
      <artifactId>edgecommons</artifactId>
      <version>${edgecommons.version}</version>
    </dependency>
  </dependencies>
</project>
"#,
        )]);
        let (changes, _) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(matches!(&changes[0], Change::Bumped { .. }), "{changes:?}");
        let text = std::fs::read_to_string(d.path().join("pom.xml")).unwrap();
        assert!(
            text.contains("<edgecommons.version>0.3.0</edgecommons.version>"),
            "{text}"
        );
        // The dependency itself must still point at the property.
        assert!(text.contains("${edgecommons.version}"), "{text}");
    }

    #[test]
    fn a_dependency_in_a_sibling_block_is_not_confused_for_ours() {
        let d = project(&[(
            "pom.xml",
            r#"<project>
  <dependencies>
    <dependency>
      <artifactId>guava</artifactId>
      <version>33.0.0</version>
    </dependency>
    <dependency>
      <artifactId>edgecommons</artifactId>
      <version>0.2.0</version>
    </dependency>
  </dependencies>
</project>
"#,
        )]);
        upgrade(d.path(), "0.3.0", false).unwrap();
        let text = std::fs::read_to_string(d.path().join("pom.xml")).unwrap();
        assert!(
            text.contains("<version>33.0.0</version>"),
            "guava must not be touched: {text}"
        );
        assert!(text.contains("<version>0.3.0</version>"), "{text}");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let d = project(&[("requirements.txt", "edgecommons==0.2.0\n")]);
        let (changes, _) = upgrade(d.path(), "0.3.0", true).unwrap();
        assert!(matches!(&changes[0], Change::Bumped { .. }));
        let text = std::fs::read_to_string(d.path().join("requirements.txt")).unwrap();
        assert!(text.contains("0.2.0"), "--dry-run must not write: {text}");
    }

    #[test]
    fn a_project_with_no_manifest_warns() {
        let d = tempfile::tempdir().unwrap();
        let (changes, report) = upgrade(d.path(), "0.3.0", false).unwrap();
        assert!(changes.is_empty());
        assert_eq!(report.warning_count(), 1);
    }

    #[test]
    fn component_version_is_a_different_thing_from_library_upgrade() {
        let d = project(&[(
            "Cargo.toml",
            "[package]\nname = \"x\"\nversion = \"1.0.0\"\n\n[dependencies]\nedgecommons = \"0.2.0\"\n",
        )]);
        set_component_version(d.path(), "2.1.0", false).unwrap();
        let text = std::fs::read_to_string(d.path().join("Cargo.toml")).unwrap();
        // The component's own version moved...
        assert!(text.contains("version = \"2.1.0\""), "{text}");
        // ...and the edgecommons library dependency did NOT. `upgrade` moves the library;
        // `version` moves the component. Conflating them is the trap this test guards.
        assert!(text.contains(r#"edgecommons = "0.2.0""#), "{text}");
    }

    #[test]
    fn editing_a_manifest_preserves_its_formatting() {
        // toml_edit is used precisely so a bump does not reformat the author's file. The
        // Python CLI's regexes had this property by accident; here it is a property we test.
        let d = project(&[(
            "Cargo.toml",
            "[package]\nname   =  \"x\"\nversion=\"1.0.0\"   # keep me\n\n[dependencies]\nedgecommons = \"0.2.0\"\n",
        )]);
        set_component_version(d.path(), "2.1.0", false).unwrap();
        let text = std::fs::read_to_string(d.path().join("Cargo.toml")).unwrap();
        assert!(text.contains("2.1.0"), "{text}");
        assert!(
            text.contains("# keep me"),
            "the trailing comment must survive: {text}"
        );
        assert!(
            text.contains("name   =  \"x\""),
            "unrelated spacing must survive: {text}"
        );
    }

    #[test]
    fn component_version_unlocks_a_scaffold_for_release() {
        // Every template ships NEXT_PATCH, and `component release` refuses it. Without
        // gdk-config.json here, an author would be told "pin a version" by a command that
        // could not pin it — which is precisely the dead end the Python CLI's `deploy --target`
        // put people in (DEF-6).
        let d = project(&[(
            "gdk-config.json",
            r#"{"component":{"com.example.Thing":{"version":"NEXT_PATCH","author":"x"}},"gdk_version":"1.6.2"}"#,
        )]);
        let changes = set_component_version(d.path(), "1.4.2", false).unwrap();
        assert!(
            matches!(&changes[0], Change::Bumped { file, from, to } if file == "gdk-config.json" && from == "NEXT_PATCH" && to == "1.4.2"),
            "{changes:?}"
        );

        let text = std::fs::read_to_string(d.path().join("gdk-config.json")).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["component"]["com.example.Thing"]["version"], "1.4.2");
        assert_eq!(
            doc["component"]["com.example.Thing"]["author"], "x",
            "siblings must survive"
        );
        assert_eq!(doc["gdk_version"], "1.6.2");
    }

    #[test]
    fn component_version_bumps_the_project_pom_not_the_dependency() {
        let d = project(&[(
            "pom.xml",
            r#"<project>
  <artifactId>thing</artifactId>
  <version>1.0.0</version>
  <dependencies>
    <dependency>
      <artifactId>edgecommons</artifactId>
      <version>0.2.0</version>
    </dependency>
  </dependencies>
</project>
"#,
        )]);
        set_component_version(d.path(), "2.0.0", false).unwrap();
        let text = std::fs::read_to_string(d.path().join("pom.xml")).unwrap();
        assert!(
            text.contains("<version>2.0.0</version>"),
            "the project version moved: {text}"
        );
        assert!(
            text.contains("<version>0.2.0</version>"),
            "the edgecommons DEPENDENCY version must NOT move — that is `upgrade`'s job: {text}"
        );
    }

    #[test]
    fn component_version_rejects_a_non_version() {
        let d = project(&[("Cargo.toml", "[package]\nname=\"x\"\nversion=\"1.0.0\"\n")]);
        assert!(matches!(
            set_component_version(d.path(), "latest", false),
            Err(Fatal::Usage(_))
        ));
        assert!(is_semver("0.3.0"));
        assert!(is_semver("1.2"));
        assert!(!is_semver("v1.2.3"));
        assert!(!is_semver(""));
    }
}
