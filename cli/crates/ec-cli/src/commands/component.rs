//! `edgecommons component new` (DESIGN-cli §5).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use ec_diag::{Fatal, Outcome, Report};
use ec_scaffold::generate::{DepSource, Inputs, default_library_subdir, generate_embedded, short_name};
use ec_scaffold::manifest::{Kind, Language, Platform};
use ec_scaffold::{catalog, discover};

use crate::cli::{self, NewArgs};

pub fn new(args: &NewArgs, quiet: bool, assume_yes: bool) -> Outcome {
    // The wizard runs when a required input is missing *and* we are on a terminal *and*
    // --yes was not passed. Off a terminal, a missing input is a usage error rather than a
    // prompt that would hang CI.
    let interactive = !assume_yes && std::io::stdin().is_terminal();

    let language = match args.language {
        Some(l) => to_lang(l),
        None if interactive => prompt_language()?,
        None => {
            return Err(Fatal::Usage(
                "a language is required: pass -l/--language (JAVA|PYTHON|RUST|TYPESCRIPT)".into(),
            ));
        }
    };

    let kind = to_kind(args.kind);

    let full_name = match &args.name {
        Some(n) => n.clone(),
        None if interactive => prompt("Fully-qualified component name (e.g. com.example.MyComponent)", None)?,
        None => return Err(Fatal::Usage("a component name is required: pass -n/--name".into())),
    };
    if full_name.trim().is_empty() {
        return Err(Fatal::Usage("the component name must not be empty".into()));
    }

    let Some(template) = catalog::find(language, kind) else {
        let available: Vec<String> = discover().iter().map(|t| t.id().to_string()).collect();
        return Err(Fatal::Usage(format!(
            "no template for {}/{}. Available: {}",
            language.as_str().to_lowercase(),
            kind.as_str(),
            available.join(", ")
        )));
    };

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
            let p = args.library_path.clone().unwrap_or_else(|| repo_root().join(subdir));
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

    let inputs = Inputs {
        full_name: full_name.clone(),
        description: args.description.clone().unwrap_or_else(|| format!("The {} component.", short_name(&full_name))),
        author: args.author.clone().unwrap_or_default(),
        platforms: platforms.clone(),
        dep_source,
        library_path,
        // Greengrass-only, and only meaningful when the GREENGRASS pack is emitted.
        bucket: String::new(),
        region: "us-east-1".into(),
    };

    let target = args.path.join(short_name(&full_name));

    if !quiet {
        println!(
            "Generating {}/{} component {} for {}",
            language.as_str().to_lowercase(),
            kind.as_str(),
            short_name(&full_name),
            platforms.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")
        );
    }

    let report = generate_embedded(&template, &inputs, &target, args.force)?;

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
        println!("{}", serde_json::to_string_pretty(&rows).unwrap_or_default());
    } else {
        println!("Templates ({}):\n", templates.len());
        let width = templates.iter().map(|t| t.id().len()).max().unwrap_or(10);
        for t in &templates {
            println!("  {:<width$}  {}", t.id(), t.manifest.description, width = width);
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
            t.manifest.platforms.iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", ")
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

fn prompt(label: &str, default: Option<&str>) -> Result<String, Fatal> {
    let suffix = default.map(|d| format!(" [{d}]")).unwrap_or_default();
    loop {
        print!("{label}{suffix}: ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
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

fn prompt_language() -> Result<Language, Fatal> {
    loop {
        let v = prompt("Language (JAVA/PYTHON/RUST/TYPESCRIPT)", None)?;
        match v.to_uppercase().as_str() {
            "JAVA" => return Ok(Language::Java),
            "PYTHON" => return Ok(Language::Python),
            "RUST" => return Ok(Language::Rust),
            "TYPESCRIPT" => return Ok(Language::Typescript),
            _ => println!("  choose one of: JAVA, PYTHON, RUST, TYPESCRIPT"),
        }
    }
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
