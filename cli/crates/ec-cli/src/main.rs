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

use crate::cli::{Cli, Command, ComponentCmd, DeploymentCmd, RegistryCmd, Shell, StudioCmd, TemplateCmd};

fn main() -> ProcExit {
    let cli = Cli::parse();
    let json = cli.json;

    // A validator that says nothing when it passes is a validator nobody trusts. `validate`
    // confirms a clean result; the other verbs print their own output and stay quiet.
    let confirm_clean = matches!(cli.command, Command::Component(ComponentCmd::Validate(_)));

    match dispatch(&cli) {
        Ok(report) => {
            emit(&report, json, cli.quiet, confirm_clean);
            // `doctor` maps a missing tool to an environment failure rather than a finding.
            let code = match &cli.command {
                Command::Doctor(_) => commands::doctor::exit_code(&report),
                _ => report.exit_code(),
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

        Command::Component(ComponentCmd::New(args)) => commands::component::new(args, cli.quiet, cli.yes),
        Command::Template(TemplateCmd::List) => commands::component::template_list(cli.json),
        Command::Template(TemplateCmd::Show { id }) => commands::component::template_show(id, cli.json),

        Command::Component(ComponentCmd::Validate(args)) => {
            if !args.path.is_dir() {
                return Err(Fatal::Usage(format!("no such component directory: {}", args.path.display())));
            }
            Ok(ec_validate::validate_project(&args.path, args.config.as_deref()))
        }

        // --- Phase P3 ---------------------------------------------------------------
        Command::Component(
            ComponentCmd::Upgrade(_)
            | ComponentCmd::Version(_)
            | ComponentCmd::Package(_)
            | ComponentCmd::Release(_),
        ) => Err(commands::not_implemented("component", "Phase P3", "§7")),
        Command::Registry(RegistryCmd::List(_) | RegistryCmd::Show { .. } | RegistryCmd::Versions { .. }) => {
            Err(commands::not_implemented("registry", "Phase P3", "§9"))
        }

        // --- Phase P4 ---------------------------------------------------------------
        Command::Deployment(
            DeploymentCmd::Validate { .. }
            | DeploymentCmd::Lock { .. }
            | DeploymentCmd::Render { .. }
            | DeploymentCmd::Plan { .. }
            | DeploymentCmd::Diff { .. }
            | DeploymentCmd::Release { .. },
        ) => Err(commands::not_implemented("deployment", "Phase P4", "§8")),

        Command::Studio(StudioCmd::Serve { repo, bind }) => {
            ec_studio::serve(&ec_studio::ServeOptions { repo: repo.clone(), bind: bind.clone() })
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

fn to_proc_exit(code: ExitCode) -> ProcExit {
    ProcExit::from(u8::try_from(code.as_i32()).unwrap_or(4))
}
