//! `edgecommons doctor` (DESIGN-cli §9).
//!
//! The Python `doctor` checked a hardcoded list of eight tools, verified no versions, omitted
//! `gh` (which its own `list-components` required) along with `docker`/`kubectl`/`helm`, and
//! **always exited 0** — which made it useless in CI (DEF-10). This one is platform-aware,
//! checks versions, and exits non-zero when something *required for a selected platform* is
//! missing.

use std::process::Command as Proc;

use ec_adapters::which;
use ec_diag::{Diagnostic, ExitCode, Outcome, Report};

use crate::cli::{DoctorArgs, Language, Platform};

/// Why a tool is needed: for a language, for a platform, or by the CLI itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    /// Required no matter what you target.
    Always,
    Language(Language),
    Platform(Platform),
}

struct Check {
    binary: &'static str,
    /// Alternative binaries; the first that resolves wins (`python3` then `python`).
    alternatives: &'static [&'static str],
    need: Need,
    why: &'static str,
    /// Arguments that make the tool print its version, when it can.
    version_args: &'static [&'static str],
    /// The minimum version we require, when there is one worth asserting.
    minimum: Option<&'static str>,
}

const CHECKS: &[Check] = &[
    Check {
        binary: "git",
        alternatives: &[],
        need: Need::Always,
        why: "clone templates and read release refs",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "gh",
        alternatives: &[],
        need: Need::Always,
        why: "read the private ecosystem registry (`registry list`)",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "cargo",
        alternatives: &[],
        need: Need::Language(Language::Rust),
        why: "build Rust components",
        version_args: &["--version"],
        minimum: Some("1.85"),
    },
    Check {
        binary: "mvn",
        alternatives: &[],
        need: Need::Language(Language::Java),
        why: "build Java components",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "java",
        alternatives: &[],
        need: Need::Language(Language::Java),
        why: "run Java components",
        version_args: &["-version"],
        minimum: Some("25"),
    },
    Check {
        binary: "python3",
        alternatives: &["python"],
        need: Need::Language(Language::Python),
        why: "run and build Python components",
        version_args: &["--version"],
        minimum: Some("3.9"),
    },
    Check {
        binary: "node",
        alternatives: &[],
        need: Need::Language(Language::Typescript),
        why: "run TypeScript components",
        version_args: &["--version"],
        minimum: Some("18"),
    },
    Check {
        binary: "npm",
        alternatives: &[],
        need: Need::Language(Language::Typescript),
        why: "install TypeScript dependencies",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "gdk",
        alternatives: &[],
        need: Need::Platform(Platform::Greengrass),
        why: "build and publish Greengrass components",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "aws",
        alternatives: &[],
        need: Need::Platform(Platform::Greengrass),
        why: "create Greengrass deployments",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "docker",
        alternatives: &[],
        need: Need::Platform(Platform::Kubernetes),
        why: "build component images",
        version_args: &["--version"],
        minimum: None,
    },
    Check {
        binary: "kubectl",
        alternatives: &[],
        need: Need::Platform(Platform::Kubernetes),
        why: "apply Kubernetes manifests",
        version_args: &["--client", "--output=yaml"],
        minimum: None,
    },
    Check {
        binary: "helm",
        alternatives: &[],
        need: Need::Platform(Platform::Kubernetes),
        why: "install the component chart",
        version_args: &["version", "--short"],
        minimum: None,
    },
    Check {
        binary: "docker",
        alternatives: &[],
        need: Need::Platform(Platform::Host),
        why: "run components and the local broker on a host",
        version_args: &["--version"],
        minimum: None,
    },
];

/// A tool's state on this machine.
struct Found {
    binary: String,
    version: Option<String>,
}

pub fn run(args: &DoctorArgs, json: bool) -> Outcome {
    let platforms: Vec<Platform> = if args.platforms.is_empty() {
        Platform::all()
    } else {
        args.platforms.clone()
    };

    let mut report = Report::new();
    let mut lines: Vec<String> = Vec::new();
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();

    for check in CHECKS {
        if !wanted(check, &platforms, args.language) {
            continue;
        }
        // `docker` is needed by both HOST and KUBERNETES; report it once.
        if seen.contains(&check.binary) {
            continue;
        }
        seen.push(check.binary);

        match locate(check) {
            Some(found) => {
                let stale = found
                    .version
                    .as_deref()
                    .zip(check.minimum)
                    .is_some_and(|(v, min)| version_is_below(v, min));

                if stale {
                    let v = found.version.clone().unwrap_or_default();
                    report.push(
                        Diagnostic::warning(
                            ec_diag::Code("EC0002"),
                            format!(
                                "{} is {v}, below the required minimum {}",
                                check.binary,
                                check.minimum.unwrap_or("?")
                            ),
                        )
                        .with_help(format!(
                            "upgrade {} — needed to {}",
                            check.binary, check.why
                        )),
                    );
                }

                lines.push(format!(
                    "  [{}] {:<10} {}{}",
                    if stale { "old" } else { " ok" },
                    check.binary,
                    found.binary,
                    found
                        .version
                        .as_deref()
                        .map(|v| format!("  ({v})"))
                        .unwrap_or_default()
                ));
                rows.push(serde_json::json!({
                    "tool": check.binary,
                    "found": true,
                    "path": found.binary,
                    "version": found.version,
                    "belowMinimum": stale,
                }));
            }
            None => {
                report.push(
                    Diagnostic::error(
                        ec_diag::Code("EC0001"),
                        format!("{} not found on PATH", check.binary),
                    )
                    .with_help(format!("needed to {}", check.why)),
                );
                lines.push(format!(
                    "  [missing] {:<10} needed to {}",
                    check.binary, check.why
                ));
                rows.push(serde_json::json!({
                    "tool": check.binary,
                    "found": false,
                    "why": check.why,
                }));
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "platforms": platforms.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
                "tools": rows,
                "ok": report.error_count() == 0,
            }))
            .unwrap_or_default()
        );
    } else {
        println!(
            "Checking prerequisites for {}:\n",
            platforms
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for l in &lines {
            println!("{l}");
        }
        println!();
    }

    Ok(report)
}

/// A missing prerequisite is an *environment* failure, not a validation finding — so
/// `doctor` maps its report onto [`ExitCode::Environment`] rather than `Findings`.
#[must_use]
pub fn exit_code(report: &Report) -> ExitCode {
    if report.error_count() > 0 {
        ExitCode::Environment
    } else {
        ExitCode::Ok
    }
}

fn wanted(check: &Check, platforms: &[Platform], language: Option<Language>) -> bool {
    match check.need {
        Need::Always => true,
        Need::Platform(p) => platforms.contains(&p),
        Need::Language(l) => language.is_none_or(|want| want == l),
    }
}

fn locate(check: &Check) -> Option<Found> {
    let candidates = std::iter::once(check.binary).chain(check.alternatives.iter().copied());
    for binary in candidates {
        if let Some(path) = which(binary) {
            return Some(Found {
                binary: path.display().to_string(),
                version: probe_version(binary, check.version_args),
            });
        }
    }
    None
}

/// Ask a tool for its version. A tool that will not answer is not an error — we simply
/// report it without one rather than inventing a failure.
fn probe_version(binary: &str, args: &[&str]) -> Option<String> {
    let out = Proc::new(binary).args(args).output().ok()?;
    // `java -version` prints to stderr; most others to stdout.
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let first = text.lines().next()?.trim().to_string();
    if first.is_empty() { None } else { Some(first) }
}

/// Compare the first dotted numeric run in `text` against `minimum` (e.g. `"1.85"`).
///
/// Deliberately lenient: tools print versions in wildly different shapes, and a doctor that
/// cries wolf on an unparseable string is worse than one that stays quiet.
fn version_is_below(text: &str, minimum: &str) -> bool {
    let Some(found) = first_version(text) else {
        return false;
    };
    let want = parse_parts(minimum);
    for (i, w) in want.iter().enumerate() {
        match found.get(i) {
            Some(f) if f > w => return false,
            Some(f) if f < w => return true,
            Some(_) => {}
            None => return true,
        }
    }
    false
}

fn first_version(text: &str) -> Option<Vec<u64>> {
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() || (ch == '.' && !current.is_empty()) {
            current.push(ch);
        } else if !current.is_empty() {
            if current.chars().any(|c| c.is_ascii_digit()) {
                return Some(parse_parts(&current));
            }
            current.clear();
        }
    }
    if current.chars().any(|c| c.is_ascii_digit()) {
        Some(parse_parts(&current))
    } else {
        None
    }
}

fn parse_parts(v: &str) -> Vec<u64> {
    v.split('.').filter_map(|p| p.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_handles_the_shapes_tools_actually_print() {
        assert!(version_is_below("cargo 1.84.0 (abc)", "1.85"));
        assert!(!version_is_below("cargo 1.96.0 (abc)", "1.85"));
        assert!(!version_is_below("cargo 1.85.0", "1.85"));
        // openjdk version "21.0.1"  -> below 25
        assert!(version_is_below(
            "openjdk version \"21.0.1\" 2023-10-17",
            "25"
        ));
        assert!(!version_is_below("openjdk version \"25\" 2025-09-16", "25"));
        // Node prints a leading v.
        assert!(version_is_below("v16.20.0", "18"));
        assert!(!version_is_below("v22.1.0", "18"));
        // Python
        assert!(version_is_below("Python 3.8.10", "3.9"));
        assert!(!version_is_below("Python 3.12.1", "3.9"));
    }

    #[test]
    fn an_unparseable_version_does_not_cry_wolf() {
        assert!(!version_is_below("some tool, no numbers here", "1.85"));
    }

    #[test]
    fn platform_selection_narrows_the_checks() {
        let host_only = [Platform::Host];
        let gdk = CHECKS.iter().find(|c| c.binary == "gdk").unwrap();
        let git = CHECKS.iter().find(|c| c.binary == "git").unwrap();
        // gdk is a Greengrass tool: not wanted for a HOST-only check...
        assert!(!wanted(gdk, &host_only, None));
        // ...but git is always needed.
        assert!(wanted(git, &host_only, None));
        // And it *is* wanted when Greengrass is selected.
        assert!(wanted(gdk, &[Platform::Greengrass], None));
    }

    #[test]
    fn language_selection_narrows_the_checks() {
        let cargo = CHECKS.iter().find(|c| c.binary == "cargo").unwrap();
        assert!(wanted(cargo, &Platform::all(), Some(Language::Rust)));
        assert!(!wanted(cargo, &Platform::all(), Some(Language::Java)));
        // With no language selected, every language's tools are checked.
        assert!(wanted(cargo, &Platform::all(), None));
    }

    #[test]
    fn gh_is_checked_because_registry_list_needs_it() {
        // The Python doctor omitted `gh` while its own list-components shelled out to it.
        assert!(CHECKS.iter().any(|c| c.binary == "gh"));
    }

    #[test]
    fn container_tooling_is_checked_for_kubernetes() {
        for tool in ["docker", "kubectl", "helm"] {
            assert!(
                CHECKS.iter().any(|c| c.binary == tool),
                "{tool} must be checked — the Python doctor omitted all three"
            );
        }
    }

    #[test]
    fn a_missing_prerequisite_is_an_environment_failure_not_a_finding() {
        let mut r = Report::new();
        assert_eq!(exit_code(&r), ExitCode::Ok);
        r.push(Diagnostic::error(
            ec_diag::Code("EC0001"),
            "gdk not found on PATH",
        ));
        // Not `Findings` (1) — a missing tool is an environment problem (3), and it is
        // non-zero, which the Python doctor never was.
        assert_eq!(exit_code(&r), ExitCode::Environment);
    }
}
