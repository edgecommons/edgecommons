//! Resolve the edgecommons library version **from the workspace**, at CLI build time.
//!
//! This is the structural fix for DEF-1. The Python CLI carried
//! `_EDGECOMMONS_VERSION = "0.1.0"` as a hand-maintained constant; when the libraries moved
//! to 0.2.0 nobody updated it, so `--dep-source registry` emitted a Cargo dependency on the
//! git tag `rust-lib/v0.1.0` — **a tag that does not exist** — and the scaffold could not
//! resolve, let alone build.
//!
//! A constant that must be remembered will eventually be forgotten. Reading the version from
//! `libs/rust/Cargo.toml` at build time means the CLI cannot ship a stale pin: if the library
//! version moves, the next CLI build moves with it, and if the file cannot be read the build
//! fails loudly rather than silently substituting a guess.

use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // cli/crates/ec-scaffold -> cli/crates -> cli -> <repo root>
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("ec-scaffold must live at <root>/cli/crates/ec-scaffold");

    let lib_manifest = repo_root.join("libs").join("rust").join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", lib_manifest.display());
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("templates").display()
    );

    let text = std::fs::read_to_string(&lib_manifest).unwrap_or_else(|e| {
        panic!(
            "cannot read the edgecommons library manifest at {}: {e}.\n\
             The CLI resolves the library version from the workspace so a generated component \
             can never be pinned to a version that does not exist (DEF-1).",
            lib_manifest.display()
        )
    });

    let version = parse_package_version(&text)
        .unwrap_or_else(|| panic!("no [package] version found in {}", lib_manifest.display()));

    println!("cargo:rustc-env=EC_LIBRARY_VERSION={version}");

    // The exact workspace commit this CLI is built from. `--dep-source pinned-rev` emits a git
    // dependency pinned to *this* rev, so the pinned templates and the library they call come
    // from the same commit by construction — the strongest correctness property available, and
    // the one the `registry` tag cannot offer (a release tag can lag the facades a template
    // calls, which is exactly what the ethernet-ip-adapter dogfooding hit). Resolved here rather
    // than from a constant for the same reason as the version above: a remembered rev drifts, a
    // build-time rev cannot. An empty value on a non-git build (a source tarball) is honest — a
    // runtime `pinned-rev` scaffold without `--library-rev` then fails loudly rather than
    // emitting `rev = ""`.
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // Re-run when HEAD moves, so a rebuild after a commit re-embeds the current rev.
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join(".git").join("HEAD").display()
    );
    println!("cargo:rustc-env=EC_LIBRARY_REV={head}");
}

/// Pull `version = "x.y.z"` from the `[package]` table — and only from it, so a dependency's
/// version cannot be mistaken for the crate's own.
fn parse_package_version(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = line.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                return Some(rest.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}
