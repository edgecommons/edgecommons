//! Release manifests and evidence bundles (DESIGN-cli §8.5; deck ch. 13 slice 2).
//!
//! `deployment release --stream artifact|config` promotes **one stream**; the manifest is the
//! `ReleaseLock`'s correlation envelope — it records both streams' identities without fusing
//! them, and each keeps its own rollback target. Deterministic: no timestamps (the Git commit
//! that lands a release carries the time), and the release hash takes the renderer version as
//! an input, so a renderer bump invalidates hashes by construction (§8.3(4)).

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::render::{RenderError, catalog_version, render};
use crate::workspace::Workspace;
use crate::{Platform, Stream};

#[derive(Debug, Error)]
pub enum ReleaseError {
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Workspace(#[from] crate::workspace::WorkspaceError),
    #[error("definition invalid: {0} error(s); run `deployment validate`")]
    Invalid(usize),
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub struct ReleaseOutput {
    /// Files relative to the release directory (`manifest.json`, `evidence.json`, `rendered/**`).
    pub files: Vec<(String, String)>,
    /// `<stream>-<short definition commit>` — deterministic, no clock involved.
    pub tag: String,
}

#[allow(clippy::too_many_arguments)]
pub fn build_release(
    ws: &Workspace,
    environment: &str,
    target: Platform,
    stream: Stream,
    config_release: &str,
    definition_commit: &str,
    warnings: &[String],
    error_count: usize,
) -> Result<ReleaseOutput, ReleaseError> {
    if error_count > 0 {
        return Err(ReleaseError::Invalid(error_count));
    }
    let output = render(ws, environment, target, config_release)?;

    let mut files_json = Vec::new();
    for f in &output.files {
        files_json.push(json!({ "path": f.path, "sha256": sha256_hex(f.text.as_bytes()) }));
    }
    let file_hash = |path: &str| -> Option<String> {
        output
            .files
            .iter()
            .find(|f| f.path == path)
            .map(|f| sha256_hex(f.text.as_bytes()))
    };

    // The two streams, correlated — never fused (§8.5).
    let mut config_stream = Map::new();
    let mut artifact_stream = Vec::new();
    let mut dev_mode = false;
    for node in &ws.definition.nodes {
        if node.config_provider.is_some() {
            config_stream.insert(
                node.key.clone(),
                json!({
                    "catalogVersion": catalog_version(ws, node, config_release)?,
                    "catalogSha256": file_hash(&format!("{}/config-catalog.json", node.key)),
                    "bootstrapSha256": file_hash(&format!("{}/config-component-config.json", node.key)),
                }),
            );
        }
        for comp in &node.components {
            let artifact = comp.artifact.as_ref();
            let pinned = artifact
                .map(|a| a.version.is_some() && a.digest.is_some())
                .unwrap_or(false);
            if !pinned {
                dev_mode = true;
            }
            artifact_stream.push(json!({
                "node": node.key,
                "component": comp.name,
                "version": artifact.and_then(|a| a.version.clone()),
                "digest": artifact.and_then(|a| a.digest.clone()),
                "source": artifact.and_then(|a| a.source.as_ref()).map(|s| json!({
                    "kind": s.kind, "repo": s.repo, "ref": s.r#ref, "features": s.features,
                })),
                "configSource": comp.config_source.as_contract_str(),
                "hotReloads": comp.config_source.hot_reloads(),
            }));
        }
    }

    let renderer = format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    // The renderer version is an input to the release hash (§8.3(4)): a renderer bump
    // invalidates by construction, as a stated, tested behavior.
    let mut hash_input = renderer.clone();
    for f in &files_json {
        hash_input.push('\n');
        hash_input.push_str(f["path"].as_str().unwrap_or_default());
        hash_input.push(' ');
        hash_input.push_str(f["sha256"].as_str().unwrap_or_default());
    }
    let release_hash = sha256_hex(hash_input.as_bytes());

    let short = definition_commit.get(..12).unwrap_or(definition_commit);
    let stream_name = match stream {
        Stream::Artifact => "artifact",
        Stream::Config => "config",
    };
    let tag = format!("{stream_name}-{short}");

    let manifest = json!({
        "release": tag,
        "promotedStream": stream_name,
        "definition": ws.definition.metadata.name,
        "environment": environment,
        "definitionCommit": definition_commit,
        "renderer": renderer,
        "releaseHash": release_hash,
        "configRelease": config_release,
        "devMode": dev_mode,
        "$comment": "The ReleaseLock correlates two independently versioned, independently rolled-back streams; promoting one never moves the other. devMode=true means at least one artifact is source-form rather than version+digest pinned - promotion to a protected environment requires full pins (deployment lock).",
        "streams": {
            "config": Value::Object(config_stream),
            "artifact": artifact_stream,
        },
        "files": files_json,
    });

    let evidence = json!({
        "schemaValidation": "pass",
        "semanticRules": "pass (S-1..S-9)",
        "warnings": warnings,
        "renderDeterminism": "re-run at the same definition commit yields byte-identical output",
    });

    let mut files = vec![
        ("manifest.json".to_string(), pretty(&manifest)),
        ("evidence.json".to_string(), pretty(&evidence)),
    ];
    for f in &output.files {
        files.push((format!("rendered/{}", f.path), f.text.clone()));
    }
    Ok(ReleaseOutput { files, tag })
}

fn pretty(value: &Value) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("JSON serialization cannot fail");
    s.push('\n');
    s
}
