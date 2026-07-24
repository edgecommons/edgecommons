//! The `edgecommons` CLI.
//!
//! One static binary: component scaffolding, validation, the ecosystem registry, and the
//! deployment kernel. Scaffolding a Java or TypeScript component no longer requires a Python
//! runtime — the adoption tax the Python CLI charged at the front door of a deliberately
//! polyglot ecosystem (RM-012).
//!
//! `main` is deliberately thin: it parses, dispatches, renders one report, and maps it to an
//! exit code. All behavior lives in the crates below it.

mod cli;
mod commands;

use std::io::Write;
use std::process::ExitCode as ProcExit;

use clap::{CommandFactory, Parser};
use ec_diag::{ExitCode, Fatal, Report};

use crate::cli::{
    Cli, Command, ComponentCmd, DeploymentCmd, RegistryCmd, Shell, StudioCmd, TemplateCmd,
};

fn main() -> ProcExit {
    let cli = Cli::parse();
    let json = cli.json;

    // A validator that says nothing when it passes is a validator nobody trusts. `validate`
    // confirms a clean result; the other verbs print their own output and stay quiet.
    let confirm_clean = matches!(cli.command, Command::Component(ComponentCmd::Validate(_)));

    // `doctor` renders its own output. In `--json` mode it emits a single document carrying its
    // tool table *and* its diagnostics — so `main` must not print a second JSON object after it.
    let doctor = matches!(cli.command, Command::Doctor(_));

    match dispatch(&cli) {
        Ok(report) => {
            if !(doctor && json) {
                emit(&report, json, cli.quiet, confirm_clean);
            }
            // `doctor` maps a missing tool to an environment failure rather than a finding.
            let code = if doctor {
                commands::doctor::exit_code(&report)
            } else {
                report.exit_code()
            };
            to_proc_exit(code)
        }
        Err(fatal) => {
            eprintln!("error: {fatal}");
            to_proc_exit(fatal.exit_code())
        }
    }
}

fn dispatch(cli: &Cli) -> Result<Report, Fatal> {
    match &cli.command {
        Command::Doctor(args) => commands::doctor::run(args, cli.json),

        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let target: clap_complete::Shell = match shell {
                Shell::Bash => clap_complete::Shell::Bash,
                Shell::Zsh => clap_complete::Shell::Zsh,
                Shell::Fish => clap_complete::Shell::Fish,
                Shell::Powershell => clap_complete::Shell::PowerShell,
                Shell::Elvish => clap_complete::Shell::Elvish,
            };
            let mut out = std::io::stdout();
            clap_complete::generate(target, &mut cmd, "edgecommons", &mut out);
            out.flush().ok();
            Ok(Report::new())
        }

        Command::Component(ComponentCmd::New(args)) => {
            commands::component::new(args, cli.quiet, cli.yes)
        }
        Command::Template(TemplateCmd::List) => commands::component::template_list(cli.json),
        Command::Template(TemplateCmd::Show { id }) => {
            commands::component::template_show(id, cli.json)
        }

        Command::Component(ComponentCmd::Validate(args)) => {
            if !args.path.is_dir() {
                return Err(Fatal::Usage(format!(
                    "no such component directory: {}",
                    args.path.display()
                )));
            }
            Ok(ec_validate::validate_project(
                &args.path,
                args.config.as_deref(),
                args.platform.map(to_deploy_platform),
            ))
        }

        Command::Component(ComponentCmd::Upgrade(args)) => {
            let (changes, report) = match (&args.to, &args.to_rev) {
                (Some(to), None) => ec_scaffold::upgrade::upgrade(&args.path, to, args.dry_run)?,
                (None, Some(rev)) => {
                    ec_scaffold::upgrade::upgrade_to_rev(&args.path, rev, args.dry_run)?
                }
                (None, None) => {
                    return Err(Fatal::Usage(
                        "pass --to <version> (release pin) or --to-rev <sha> (git-rev pin)".into(),
                    ));
                }
                (Some(_), Some(_)) => unreachable!("clap declares these mutually exclusive"),
            };
            for c in &changes {
                println!(
                    "{}{}",
                    if args.dry_run { "[dry-run] " } else { "" },
                    c.describe(ec_scaffold::upgrade::Subject::Library)
                );
            }
            Ok(report)
        }
        Command::Component(ComponentCmd::Version(args)) => {
            let changes =
                ec_scaffold::upgrade::set_component_version(&args.path, &args.to, args.dry_run)?;
            if changes.is_empty() {
                return Err(Fatal::Usage(
                    "no manifest declaring a component version found (Cargo.toml, package.json)"
                        .into(),
                ));
            }
            for c in &changes {
                println!(
                    "{}{}",
                    if args.dry_run { "[dry-run] " } else { "" },
                    c.describe(ec_scaffold::upgrade::Subject::Component)
                );
            }
            Ok(Report::new())
        }
        Command::Component(ComponentCmd::Package(args)) => {
            commands::release::package(args, cli.quiet)
        }
        Command::Component(ComponentCmd::Release(args)) => {
            commands::release::release(args, cli.quiet)
        }

        Command::Registry(RegistryCmd::List(args)) => commands::registry::list(args, cli.json),
        Command::Registry(RegistryCmd::Show { name, source }) => {
            commands::registry::show(name, source.as_deref(), cli.json)
        }
        Command::Registry(RegistryCmd::Versions { name, source }) => {
            commands::registry::versions(name, source.as_deref())
        }

        Command::Deployment(DeploymentCmd::Validate { definition }) => {
            commands::deployment::validate(definition)
        }
        Command::Deployment(DeploymentCmd::Render {
            definition,
            env,
            target,
        }) => commands::deployment::render_cmd(definition, env, *target, cli.quiet),
        Command::Deployment(DeploymentCmd::Plan {
            definition,
            env,
            target,
        }) => commands::deployment::plan(definition, env, *target),
        Command::Deployment(DeploymentCmd::Release { definition, stream }) => {
            commands::deployment::release_cmd(definition, *stream, cli.quiet)
        }
        Command::Deployment(DeploymentCmd::Lock { definition, source }) => {
            commands::deployment::lock(definition, source.as_deref(), cli.quiet)
        }
        // Consequence-grouped diff is not built yet.
        Command::Deployment(DeploymentCmd::Diff { .. }) => {
            Err(commands::not_implemented("deployment"))
        }
        Command::Deployment(DeploymentCmd::Draft(cmd)) => match cmd {
            crate::cli::DraftCmd::Open { title, repo, base } => {
                commands::draft::open(title, repo, base)
            }
            crate::cli::DraftCmd::Edit {
                git_ref,
                path,
                contents,
                repo,
            } => commands::draft::edit(git_ref, path, contents, repo),
            crate::cli::DraftCmd::List { repo } => commands::draft::list(repo),
            crate::cli::DraftCmd::Status {
                git_ref,
                repo,
                profile,
                main,
            } => commands::draft::status(git_ref, repo, profile.as_deref(), main),
        },

        Command::Studio(StudioCmd::Serve { repo, bind }) => {
            ec_studio::serve(&ec_studio::ServeOptions {
                repo: repo.clone(),
                bind: bind.clone(),
            })
            .map(|()| Report::new())
        }
    }
}

/// Render the report. `doctor` prints its own table, so an empty report stays silent there.
fn emit(report: &Report, json: bool, quiet: bool, confirm_clean: bool) {
    if report.is_empty() && !confirm_clean {
        return;
    }
    if json {
        println!("{}", report.render_json());
    } else if !quiet {
        print!("{}", report.render_human());
    }
}

/// The CLI's platform enum, in the kernel's vocabulary.
fn to_deploy_platform(p: crate::cli::Platform) -> ec_deploy::Platform {
    match p {
        crate::cli::Platform::Greengrass => ec_deploy::Platform::Greengrass,
        crate::cli::Platform::Host => ec_deploy::Platform::Host,
        crate::cli::Platform::Kubernetes => ec_deploy::Platform::Kubernetes,
    }
}

fn to_proc_exit(code: ExitCode) -> ProcExit {
    ProcExit::from(u8::try_from(code.as_i32()).unwrap_or(4))
}
