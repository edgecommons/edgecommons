//! `edgecommons component new` (DESIGN-cli §5).

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use ec_diag::{Fatal, Outcome, Report};
use ec_scaffold::generate::{
    DepSource, EDGECOMMONS_REV, Inputs, bin_name, default_library_subdir, generate_embedded,
    short_name,
};
use ec_scaffold::licenses;
use ec_scaffold::manifest::{Kind, Language, Platform};
use ec_scaffold::{catalog, discover};

/// The sentinel written into `gdk-config.json`'s publish bucket when a Greengrass scaffold has
/// none. A plain literal (so it does not trip the `<<TOKEN>>` drift gate) that is visible in the
/// file and that `component validate` rejects, so the miss is caught at authoring/CI rather than
/// at `gdk component publish` weeks later.
pub const ARTIFACT_BUCKET_SENTINEL: &str = "edgecommons-set-artifact-bucket";

use crate::cli::{self, NewArgs};

pub fn new(args: &NewArgs, quiet: bool, assume_yes: bool) -> Outcome {
    // The wizard runs when a required input is missing *and* we are on a terminal *and*
    // --yes was not passed. Off a terminal, a missing input is a usage error rather than a
    // prompt that would hang CI.
    let interactive = !assume_yes && std::io::stdin().is_terminal();

    let full_name = match &args.name {
        Some(n) => n.clone(),
        None if interactive => prompt(
            "Fully-qualified component name (e.g. com.example.MyComponent)",
            None,
        )?,
        None => {
            return Err(Fatal::Usage(
                "a component name is required: pass -n/--name".into(),
            ));
        }
    };
    if full_name.trim().is_empty() {
        return Err(Fatal::Usage("the component name must not be empty".into()));
    }

    // Resolve the template. A custom source (a directory, or a git URL) carries its own
    // manifest, and that manifest — not the command line — declares its language and kind: a
    // template is a template wherever it comes from, and the same generation path runs for all
    // of them. Only the embedded catalog needs `-l`/`-k` to *find* a template.
    let (template, source_files) = match (&args.template_dir, &args.template_git) {
        (Some(dir), None) => {
            let (t, files) = template_from_dir(dir)?;
            (t, Some(files))
        }
        (None, Some(url)) => {
            let tmp = tempfile::tempdir().map_err(|e| Fatal::Internal(e.to_string()))?;
            clone(url, tmp.path())?;
            let (t, files) = template_from_dir(tmp.path())?;
            (t, Some(files))
        }
        (Some(_), Some(_)) => unreachable!("clap declares these mutually exclusive"),
        (None, None) => {
            let language = match args.language {
                Some(l) => to_lang(l),
                None if interactive => prompt_language()?,
                None => {
                    return Err(Fatal::Usage(
                        "a language is required: pass -l/--language (JAVA|PYTHON|RUST|TYPESCRIPT)"
                            .into(),
                    ));
                }
            };
            let kind = to_kind(args.kind);
            let Some(t) = catalog::find(language, kind) else {
                let available: Vec<String> =
                    discover().iter().map(|t| t.id().to_string()).collect();
                return Err(Fatal::Usage(format!(
                    "no template for {}/{}. Available: {}",
                    language.as_str().to_lowercase(),
                    kind.as_str(),
                    available.join(", ")
                )));
            };
            (t, None)
        }
    };

    let language = template.manifest.language;
    let kind = template.manifest.kind;

    let platforms: Vec<Platform> = if args.platforms.is_empty() {
        template.manifest.platforms.clone()
    } else {
        args.platforms.iter().map(|p| to_platform(*p)).collect()
    };

    let dep_source = match args.dep_source {
        cli::DepSource::Local => DepSource::Local,
        cli::DepSource::Registry => DepSource::Registry,
        cli::DepSource::PinnedRev => DepSource::PinnedRev,
    };

    // pinned-rev is a Rust/Python capability: Maven and npm cannot express a git dependency on
    // the libs/<lang> subdirectory of the edgecommons monorepo. Reject Java/TS *before*
    // generation, naming the honest capability limit rather than silently falling back.
    if dep_source == DepSource::PinnedRev
        && matches!(language, Language::Java | Language::Typescript)
    {
        return Err(Fatal::Usage(format!(
            "--dep-source pinned-rev is not available for {}: Maven and npm cannot express a git \
             dependency on a subdirectory of the edgecommons monorepo. Use --dep-source registry \
             (the published artifact) or local (a sibling checkout).",
            language.as_str()
        )));
    }

    // The git revision to pin to (pinned-rev only): --library-rev if given, else the commit this
    // CLI was built from. If both are empty (a non-git build with no flag), a pinned-rev scaffold
    // cannot emit a valid pin, so it is an environment error naming the fix rather than a
    // `rev = ""` the author discovers at build time.
    let library_rev = if dep_source == DepSource::PinnedRev {
        let resolved = args
            .library_rev
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| (!EDGECOMMONS_REV.is_empty()).then(|| EDGECOMMONS_REV.to_string()));
        match resolved {
            Some(r) => Some(r),
            None => {
                return Err(Fatal::Environment(
                    "--dep-source pinned-rev needs a git revision, but this CLI was built outside \
                     a git checkout so none is embedded. Pass --library-rev <sha>."
                        .into(),
                ));
            }
        }
    } else {
        None
    };

    // `--bin-name`/`--crate-name` overrides the derived kebab name (and must itself be a valid
    // crate/binary token). One flag for all languages — a per-language pair would be parity drift.
    let bin_override = match &args.bin_name {
        Some(b) => {
            if !is_valid_bin_name(b) {
                return Err(Fatal::Usage(format!(
                    "--bin-name `{b}` is not a valid crate/binary name: expected \
                     ^[a-z0-9][a-z0-9-]*$ (lowercase letters, digits, and hyphens, starting with \
                     a letter or digit)."
                )));
            }
            Some(b.clone())
        }
        None => None,
    };

    // The local sibling path: for `local` it *is* the dependency and must exist; for `pinned-rev`
    // it is the gitignored `.cargo` dev override, which is emitted even if the path is absent on
    // this machine (it is dev tooling, and the file's comment says to fix the path).
    let library_path = match (dep_source, default_library_subdir(language)) {
        (DepSource::Local, Some(subdir)) => {
            let p = args
                .library_path
                .clone()
                .unwrap_or_else(|| repo_root().join(subdir));
            if !p.is_dir() {
                return Err(Fatal::Usage(format!(
                    "{} components with --dep-source local need the edgecommons library, but `{}` \
                     does not exist. Pass --library-path <path>, or use --dep-source registry.",
                    language.as_str(),
                    p.display()
                )));
            }
            Some(p)
        }
        (DepSource::PinnedRev, Some(subdir)) => Some(
            args.library_path
                .clone()
                .unwrap_or_else(|| repo_root().join(subdir)),
        ),
        _ => None,
    };

    let license = args.license.spdx();

    let default_description = format!("The {} component.", short_name(&full_name));
    let description = match &args.description {
        Some(d) => d.clone(),
        None if interactive => prompt("Description", Some(&default_description))?,
        None => default_description,
    };

    let author = match &args.author {
        Some(a) => a.clone(),
        None if interactive => prompt("Author", Some(""))?,
        None => String::new(),
    };

    // BUCKET/REGION are Greengrass-only, and only asked for when the GREENGRASS pack is
    // actually being emitted — the AWS-era CLI asked every author for an S3 bucket regardless
    // of what they were building. Without one, the generated gdk-config.json cannot publish, so
    // its absence is reported rather than silently baked in.
    let greengrass = platforms.contains(&Platform::Greengrass);
    let bucket = match (&args.bucket, greengrass, interactive) {
        (Some(b), _, _) => b.clone(),
        (None, true, true) => prompt("S3 bucket for Greengrass artifacts", Some(""))?,
        _ => String::new(),
    };
    // A Greengrass scaffold with no bucket gets the visible sentinel rather than an empty string:
    // an empty publish bucket is an invisible landmine, whereas the sentinel is obvious in the
    // file, prints in "Next steps", and is a hard error at `component validate`.
    let bucket_missing = greengrass && bucket.trim().is_empty();
    let effective_bucket = if bucket_missing {
        ARTIFACT_BUCKET_SENTINEL.to_string()
    } else {
        bucket
    };

    let inputs = Inputs {
        full_name: full_name.clone(),
        description,
        author,
        platforms: platforms.clone(),
        bin_name: bin_override.clone(),
        dep_source,
        library_path,
        library_rev,
        license,
        bucket: effective_bucket,
        region: args.region.clone(),
    };

    // The output directory: `--dir` wins outright; otherwise the derived kebab name (honoring a
    // `--bin-name` override) under `--path`.
    let derived_dir = bin_override
        .clone()
        .unwrap_or_else(|| bin_name(&short_name(&full_name)));
    let target = args
        .dir
        .clone()
        .unwrap_or_else(|| args.path.join(&derived_dir));

    if !quiet {
        println!(
            "Generating {}/{} component {} for {}",
            language.as_str().to_lowercase(),
            kind.as_str(),
            short_name(&full_name),
            platforms
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let mut report = match source_files {
        // A custom template's files were read when it was resolved.
        Some(files) => {
            ec_scaffold::generate::generate(&template, &inputs, &target, args.force, files)?
        }
        // The embedded template — the offline default.
        None => generate_embedded(&template, &inputs, &target, args.force)?,
    };

    // The license (SD-4, D-CLI-21): the author's choice, not EdgeCommons'. Applied after generation
    // so it lands in the finished tree. `--license <spdx>` writes the LICENSE text and leaves the
    // manifest's populated `license` field. `--license none` (the default) writes no LICENSE file
    // AND removes the now-empty license field the template carries (`license = "<<LICENSE>>"` ->
    // `license = ""`), so a scaffold makes **no** license claim rather than an empty one.
    if report.error_count() == 0 {
        match license {
            Some(spdx) => write_license(&target, spdx)?,
            None => strip_empty_license_fields(&target),
        }
    }

    if bucket_missing {
        report.push(
            ec_diag::Diagnostic::warning(
                ec_diag::EC4005_NO_ARTIFACT_BUCKET,
                format!(
                    "no artifact bucket: gdk-config.json carries the sentinel \
                     `{ARTIFACT_BUCKET_SENTINEL}` and cannot publish until it is set"
                ),
            )
            .with_file(target.join("gdk-config.json"))
            .with_help("set `publish.bucket` in gdk-config.json, or re-scaffold with -b/--bucket"),
        );
    }

    if report.error_count() == 0 && !quiet {
        println!("Done. Component generated at: {}", target.display());
        print_next_steps(bucket_missing, language, &target);
    }
    Ok(report)
}

/// Whether a string is a valid crate/binary name: `^[a-z0-9][a-z0-9-]*[a-z0-9]$` (or a single
/// `[a-z0-9]`). A trailing hyphen is rejected — it is an invalid crate name and never what the
/// author meant.
fn is_valid_bin_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.ends_with('-')
}

/// Remove an **empty** license claim from the generated manifests (the `--license none` case).
///
/// The templates carry a tokenized `license` field (`license = "<<LICENSE>>"`) so `--license <spdx>`
/// can populate it; with `none` the token resolves to `""`, and an empty `license = ""` reads as a
/// deliberate (empty) claim. SD-4/D-CLI-21 want **no** claim, so the field is stripped. Best-effort:
/// a manifest the template does not ship is simply skipped.
fn strip_empty_license_fields(target: &std::path::Path) {
    // Single-line `key = ""` / `"key": ""` forms: Cargo.toml, pyproject.toml (TOML) + package.json.
    for (name, is_empty_license_line) in [
        ("Cargo.toml", toml_license_is_empty as fn(&str) -> bool),
        ("pyproject.toml", toml_license_is_empty),
        ("package.json", json_license_is_empty),
    ] {
        let path = target.join(name);
        if let Ok(text) = std::fs::read_to_string(&path) {
            let kept: Vec<&str> = text.lines().filter(|l| !is_empty_license_line(l)).collect();
            let mut out = kept.join("\n");
            if text.ends_with('\n') {
                out.push('\n');
            }
            if out != text {
                let _ = std::fs::write(&path, out);
            }
        }
    }
    // Multi-line `<licenses><license><name></name></license></licenses>` block: pom.xml.
    let pom = target.join("pom.xml");
    if let Ok(text) = std::fs::read_to_string(&pom)
        && let Some(stripped) = strip_empty_pom_licenses(&text)
    {
        let _ = std::fs::write(&pom, stripped);
    }
}

/// A TOML `license = ""` line (any surrounding whitespace).
fn toml_license_is_empty(line: &str) -> bool {
    let t = line.trim();
    t == "license = \"\"" || t == "license=\"\""
}

/// A JSON `"license": ""` line (with or without a trailing comma).
fn json_license_is_empty(line: &str) -> bool {
    let t = line.trim().trim_end_matches(',').trim();
    t == "\"license\": \"\"" || t == "\"license\":\"\""
}

/// Remove a `<licenses>` block whose only `<license>` has an empty `<name>` (the `--license none`
/// pom). Returns the new text if a block was removed, else `None`.
fn strip_empty_pom_licenses(text: &str) -> Option<String> {
    let start = text.find("<licenses>")?;
    let end = text[start..]
        .find("</licenses>")
        .map(|e| start + e + "</licenses>".len())?;
    let block = &text[start..end];
    // Only strip when the license name is empty (an unset `--license none`), never a real license.
    if !(block.contains("<name></name>") || block.contains("<name/>")) {
        return None;
    }
    // Also swallow the line's leading indentation and the trailing newline, so no blank line is left.
    let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
    let mut after = end;
    if text[after..].starts_with('\n') {
        after += 1;
    }
    Some(format!("{}{}", &text[..line_start], &text[after..]))
}

/// Write the chosen license's canonical text into the generated component.
fn write_license(target: &std::path::Path, spdx: &str) -> Result<(), Fatal> {
    let text = licenses::text(spdx)
        .ok_or_else(|| Fatal::Internal(format!("no embedded license text for `{spdx}`")))?;
    std::fs::write(target.join("LICENSE"), text)
        .map_err(|e| Fatal::Internal(format!("writing LICENSE: {e}")))
}

/// The "Next steps" epilogue printed after a successful scaffold (non-quiet). It names the
/// per-repo work the dogfooding had to re-derive by hand: setting the bucket, committing the
/// first lockfile, and wiring org CI secrets.
fn print_next_steps(bucket_missing: bool, language: Language, target: &std::path::Path) {
    println!("\nNext steps:");
    if bucket_missing {
        println!(
            "  - set `publish.bucket` in {}/gdk-config.json (currently the sentinel \
             `{ARTIFACT_BUCKET_SENTINEL}`).",
            target.display()
        );
    }
    match language {
        Language::Rust => println!(
            "  - build once and commit the generated Cargo.lock so CI builds are reproducible."
        ),
        Language::Typescript => println!(
            "  - build once and commit the generated package-lock.json so CI builds are \
             reproducible."
        ),
        _ => {}
    }
    println!("  - push the repo and wire it into the org CI secrets (EDGECOMMONS_READ_TOKEN).");
}

pub fn template_list(json: bool) -> Outcome {
    let templates = discover();
    if json {
        let rows: Vec<_> = templates
            .iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id(),
                    "language": t.manifest.language.as_str(),
                    "kind": t.manifest.kind.as_str(),
                    "platforms": t.manifest.platforms.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                    "description": t.manifest.description,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
    } else {
        println!("Templates ({}):\n", templates.len());
        let width = templates.iter().map(|t| t.id().len()).max().unwrap_or(10);
        for t in &templates {
            println!(
                "  {:<width$}  {}",
                t.id(),
                t.manifest.description,
                width = width
            );
        }
        println!("\nScaffold one with: edgecommons component new -l <LANG> -k <KIND> -n <name>");
    }
    Ok(Report::new())
}

pub fn template_show(id: &str, json: bool) -> Outcome {
    let Some(t) = discover().into_iter().find(|t| t.id() == id) else {
        return Err(Fatal::Usage(format!(
            "no template `{id}`. Run `edgecommons template list` to see the matrix."
        )));
    };
    let files = catalog::files(&t.dir);
    let mut names: Vec<&str> = files
        .iter()
        .map(|(p, _)| p.as_str())
        .filter(|p| *p != catalog::MANIFEST_NAME)
        .collect();
    names.sort_unstable();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": t.id(),
                "language": t.manifest.language.as_str(),
                "kind": t.manifest.kind.as_str(),
                "description": t.manifest.description,
                "platforms": t.manifest.platforms.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                "packs": t.manifest.packs.iter().map(|(p, v)| (p.as_str(), v.clone())).collect::<std::collections::BTreeMap<_, _>>(),
                "files": names,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("{}  —  {}\n", t.id(), t.manifest.description);
        println!(
            "  platforms: {}",
            t.manifest
                .platforms
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for (platform, paths) in &t.manifest.packs {
            println!("  {} pack: {}", platform.as_str(), paths.join(", "));
        }
        println!("\n  files ({}):", names.len());
        for n in names {
            println!("    {n}");
        }
    }
    Ok(Report::new())
}

/// A template plus the bytes of every file in it.
type LoadedTemplate = (ec_scaffold::Template, Vec<(String, Vec<u8>)>);

/// Read a template from a directory on disk: its manifest, and every file in it.
///
/// The same manifest-v2 contract the embedded templates obey — a template is a template
/// wherever it comes from, so a fork or a work-in-progress template is exercised by exactly the
/// code path that ships.
fn template_from_dir(dir: &Path) -> Result<LoadedTemplate, Fatal> {
    let manifest_path = dir.join(catalog::MANIFEST_NAME);
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        Fatal::Usage(format!(
            "`{}` is not a template: cannot read {} ({e})",
            dir.display(),
            catalog::MANIFEST_NAME
        ))
    })?;
    let manifest = ec_scaffold::Manifest::parse(&text)
        .map_err(|e| Fatal::Usage(format!("{}: {e}", manifest_path.display())))?;

    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        // A template directory may be a git checkout; its history is not part of the template.
        if entry.path().components().any(|c| c.as_os_str() == ".git") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(dir)
            .map_err(|e| Fatal::Internal(e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(entry.path()).map_err(|e| Fatal::Internal(e.to_string()))?;
        files.push((rel, bytes));
    }

    let template = ec_scaffold::Template {
        dir: dir.display().to_string(),
        manifest,
    };
    Ok((template, files))
}

/// Clone a template repository. The one network access `component new` makes, and only when
/// explicitly asked for with `--template-git`.
fn clone(url: &str, into: &Path) -> Result<(), Fatal> {
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(into)
        .status()
        .map_err(|e| Fatal::Environment(format!("git failed to start: {e}")))?;
    if !status.success() {
        return Err(Fatal::Environment(format!(
            "git clone {url} failed with {status}"
        )));
    }
    Ok(())
}

/// The monorepo this CLI was built from — used to default a local library path.
fn repo_root() -> PathBuf {
    // <root>/cli/crates/ec-cli/  ->  <root>
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Ask a question. Takes its input rather than reaching for `stdin`, so the wizard — the part
/// of the CLI a user actually converses with — is testable rather than merely hoped-for.
fn ask(r: &mut impl BufRead, label: &str, default: Option<&str>) -> Result<String, Fatal> {
    let suffix = default.map(|d| format!(" [{d}]")).unwrap_or_default();
    loop {
        print!("{label}{suffix}: ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            // EOF: the caller closed the input. Take the default if there is one rather than
            // spinning forever on an empty read.
            return default.map(str::to_string).ok_or_else(|| {
                Fatal::Usage(format!("`{label}` is required, and there is no more input"))
            });
        }
        let v = line.trim();
        if !v.is_empty() {
            return Ok(v.to_string());
        }
        if let Some(d) = default {
            return Ok(d.to_string());
        }
        println!("  (a value is required)");
    }
}

fn prompt(label: &str, default: Option<&str>) -> Result<String, Fatal> {
    ask(&mut std::io::stdin().lock(), label, default)
}

fn ask_language(r: &mut impl BufRead) -> Result<Language, Fatal> {
    loop {
        let v = ask(r, "Language (JAVA/PYTHON/RUST/TYPESCRIPT)", None)?;
        match v.to_uppercase().as_str() {
            "JAVA" => return Ok(Language::Java),
            "PYTHON" => return Ok(Language::Python),
            "RUST" => return Ok(Language::Rust),
            "TYPESCRIPT" => return Ok(Language::Typescript),
            _ => println!("  choose one of: JAVA, PYTHON, RUST, TYPESCRIPT"),
        }
    }
}

fn prompt_language() -> Result<Language, Fatal> {
    ask_language(&mut std::io::stdin().lock())
}

fn to_lang(l: cli::Language) -> Language {
    match l {
        cli::Language::Java => Language::Java,
        cli::Language::Python => Language::Python,
        cli::Language::Rust => Language::Rust,
        cli::Language::Typescript => Language::Typescript,
    }
}

fn to_kind(k: cli::Kind) -> Kind {
    match k {
        cli::Kind::Service => Kind::Service,
        cli::Kind::ProtocolAdapter => Kind::ProtocolAdapter,
        cli::Kind::Processor => Kind::Processor,
        cli::Kind::Sink => Kind::Sink,
    }
}

fn to_platform(p: cli::Platform) -> Platform {
    match p {
        cli::Platform::Greengrass => Platform::Greengrass,
        cli::Platform::Host => Platform::Host,
        cli::Platform::Kubernetes => Platform::Kubernetes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(s: &str) -> std::io::Cursor<Vec<u8>> {
        std::io::Cursor::new(s.as_bytes().to_vec())
    }

    #[test]
    fn bin_name_validation_matches_the_crate_name_grammar() {
        for good in ["my-bin", "adapter2", "a", "ethernet-ip-adapter", "0x"] {
            assert!(is_valid_bin_name(good), "`{good}` should be valid");
        }
        // The grammar (^[a-z0-9][a-z0-9-]*$) rejects uppercase, leading hyphens, underscores,
        // spaces, empties, and non-ASCII. (A trailing hyphen is permitted by the grammar.)
        for bad in [
            "My-Bin",
            "-leading",
            "under_score",
            "has space",
            "",
            "Ünïcode",
        ] {
            assert!(!is_valid_bin_name(bad), "`{bad}` should be rejected");
        }
    }

    #[test]
    fn a_prompt_takes_the_answer() {
        let mut r = input(
            "com.example.Thing
",
        );
        assert_eq!(ask(&mut r, "Name", None).unwrap(), "com.example.Thing");
    }

    #[test]
    fn an_empty_answer_takes_the_default() {
        let mut r = input(
            "
",
        );
        assert_eq!(
            ask(&mut r, "Description", Some("A component.")).unwrap(),
            "A component."
        );
    }

    #[test]
    fn a_required_answer_is_re_asked_until_given() {
        // Two blank lines, then a real answer: the wizard must keep asking rather than accept
        // an empty required value.
        let mut r = input(
            "

finally
",
        );
        assert_eq!(ask(&mut r, "Name", None).unwrap(), "finally");
    }

    #[test]
    fn eof_on_a_required_answer_is_a_usage_error_not_an_infinite_loop() {
        let mut r = input("");
        assert!(matches!(ask(&mut r, "Name", None), Err(Fatal::Usage(_))));
    }

    #[test]
    fn eof_with_a_default_takes_the_default() {
        let mut r = input("");
        assert_eq!(ask(&mut r, "Author", Some("")).unwrap(), "");
    }

    #[test]
    fn the_language_prompt_re_asks_on_a_bad_answer() {
        let mut r = input(
            "COBOL
rust
",
        );
        assert_eq!(ask_language(&mut r).unwrap(), Language::Rust);
    }

    #[test]
    fn the_language_prompt_is_case_insensitive() {
        for (given, want) in [
            ("java", Language::Java),
            ("PYTHON", Language::Python),
            ("TypeScript", Language::Typescript),
        ] {
            let mut r = input(&format!(
                "{given}
"
            ));
            assert_eq!(ask_language(&mut r).unwrap(), want);
        }
    }
}
