//! `edgecommons component new` (DESIGN-cli §5).

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use ec_diag::{Fatal, Outcome, Report};
use ec_scaffold::generate::{
    DepSource, Inputs, default_library_subdir, generate_embedded, short_name,
};
use ec_scaffold::manifest::{Kind, Language, Platform};
use ec_scaffold::{catalog, discover};

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
    };

    // A local path dependency needs a real library path. Resolve it relative to the monorepo
    // this CLI was built from, so the common case needs no flag.
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
        _ => None,
    };

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

    let inputs = Inputs {
        full_name: full_name.clone(),
        description,
        author,
        platforms: platforms.clone(),
        dep_source,
        library_path,
        bucket: bucket.clone(),
        region: args.region.clone(),
    };

    let target = args.path.join(short_name(&full_name));

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

    if greengrass && bucket.is_empty() {
        report.push(
            ec_diag::Diagnostic::warning(
                ec_diag::EC4005_NO_ARTIFACT_BUCKET,
                "no artifact bucket: gdk-config.json cannot publish as generated".to_string(),
            )
            .with_file(target.join("gdk-config.json"))
            .with_help("set `publish.bucket` in gdk-config.json, or pass -b/--bucket"),
        );
    }

    if report.error_count() == 0 && !quiet {
        println!("Done. Component generated at: {}", target.display());
    }
    Ok(report)
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
