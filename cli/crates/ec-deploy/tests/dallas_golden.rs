//! The Dallas golden test: the HOST renderer's byte-for-byte acceptance gate (DESIGN-cli
//! §8.3(3)). The fixture under `tests/fixtures/dallas` is the Dallas bottling site — the same
//! definition that generates the adopted `bottling-company-test/sites/dallas-site` — and
//! `golden/` is its committed rendered output. Any change to the renderer, the model, or the
//! fixture that alters a single byte fails here.
//!
//! This is the same proof the Deployment Studio repo carried while the kernel lived there;
//! it moved into core with the kernel so the tool and its golden fixture live together.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ec_deploy::Platform;
use ec_deploy::render::render;
use ec_deploy::workspace::{Workspace, parse_authored, referenced_paths};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dallas")
}

/// Load the fixture the way an adapter would, but from this crate's test tree: parse the authored
/// (topology + profiles) definition and merge in the `host` profile to get the effective
/// definition the HOST renderer consumes.
fn load() -> Workspace {
    let root = fixture_dir();
    let definition_text = std::fs::read_to_string(root.join("definition.yaml")).unwrap();
    let authored = parse_authored(&definition_text).expect("fixture definition parses");
    let doc = authored.effective("host").expect("host profile merges");
    let mut files = BTreeMap::new();
    for rel in referenced_paths(&doc) {
        let text = std::fs::read_to_string(root.join(&rel))
            .unwrap_or_else(|e| panic!("reading referenced {rel}: {e}"));
        files.insert(rel, text);
    }
    Workspace {
        definition: doc,
        files,
    }
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

#[test]
fn dallas_renders_byte_for_byte_to_the_committed_golden() {
    let ws = load();
    let output = render(&ws, "local", Platform::Host, "initial").expect("render succeeds");
    let golden_root = fixture_dir().join("golden");

    let mut mismatches = Vec::new();
    let mut rendered_paths = Vec::new();
    for f in &output.files {
        rendered_paths.push(f.path.clone());
        let golden = golden_root.join(&f.path);
        match std::fs::read_to_string(&golden) {
            Ok(want) => {
                if normalize(&want) != normalize(&f.text) {
                    mismatches.push(format!("{}: rendered bytes differ from golden", f.path));
                }
            }
            Err(_) => mismatches.push(format!(
                "{}: no golden file (renderer produced a new file)",
                f.path
            )),
        }
    }

    // Every golden file must be produced (no silently dropped output).
    for entry in walk(&golden_root) {
        let rel = entry
            .strip_prefix(&golden_root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !rendered_paths.contains(&rel) {
            mismatches.push(format!(
                "{rel}: in golden but the renderer no longer produces it"
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "Dallas golden mismatch ({} file(s)):\n{}\n\nIf the renderer changed intentionally, \
         regenerate: `edgecommons deployment render tests/fixtures/dallas/definition.yaml \
         --env local --target HOST` and move render/host over golden/.",
        mismatches.len(),
        mismatches.join("\n")
    );
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
