//! The Deployment Studio server (DESIGN-cli §8.4, deck ch. 12): an `axum` service over the **same
//! kernel the CLI uses**. The server adds branch/draft orchestration, the UI, evidence correlation,
//! and access control — never a capability the CLI lacks. Git is the only durable state.
//!
//! These are the **read-only** cuts of slice 5 (deck ch. 13): the server loads a definition from
//! the repo and serves, for any profile, the config layers, the render/plan, the **evidence
//! correlation** envelope a release would carry (REVIEW #13 — Studio holds intent and adjudicates
//! from evidence), and the **access control** rendering of the repo's `CODEOWNERS` (REVIEW #10 —
//! who must review a change is a rendering of Git-host review state, not a parallel system).
//! Authoring and branch/draft orchestration — the write path — are later cuts; nothing here writes.
//! No cloud SDK sits above the port boundary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use ec_deploy::codeowners::CodeOwners;
use ec_deploy::release::build_release;
use ec_deploy::render::render;
use ec_deploy::validate::validate as kernel_validate;
use ec_deploy::{Platform, Stream};
use ec_diag::Fatal;
use include_dir::{Dir, include_dir};
use serde::Serialize;
use serde_json::{Value, json};

/// The React + Carbon single-page app, built by `ui/` (vite) and embedded so the Studio ships as a
/// single static binary (deck ch. 12). Rebuild with `npm --prefix crates/ec-studio/ui run build`.
static UI: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/ui/dist");

/// Where the server serves from, and what it serves against.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// The Git repository (working tree) holding desired state — a directory containing a
    /// `definition.yaml`. Git is the database; SQLite would be a rebuildable cache; there is no
    /// third datastore.
    pub repo: String,
    pub bind: String,
}

struct AppState {
    loaded: ec_adapters::LoadedWorkspace,
}

/// Serve the Studio UI + read-only API over the same kernel the CLI uses.
///
/// # Errors
///
/// Returns [`Fatal::Environment`] if the repo has no readable `definition.yaml`, [`Fatal::Usage`]
/// if it does not parse, or [`Fatal::Internal`] if the server cannot bind or run.
pub fn serve(opts: &ServeOptions) -> Result<(), Fatal> {
    let definition = definition_path(&opts.repo);
    let loaded = ec_adapters::load_workspace(&definition).map_err(|e| {
        Fatal::Environment(format!(
            "no readable deployment definition at {}: {e}",
            definition.display()
        ))
    })?;
    let state = Arc::new(AppState { loaded });

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Fatal::Internal(format!("starting the async runtime: {e}")))?;

    runtime.block_on(async move {
        let app = router(state);
        let listener = tokio::net::TcpListener::bind(&opts.bind)
            .await
            .map_err(|e| Fatal::Internal(format!("binding {}: {e}", opts.bind)))?;
        let addr = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| opts.bind.clone());
        println!(
            "Deployment Studio (read-only) serving {} at http://{addr}",
            opts.repo
        );
        axum::serve(listener, app)
            .await
            .map_err(|e| Fatal::Internal(format!("serving: {e}")))
    })
}

/// The definition a repo path points at: the path itself if it is a file, else `<repo>/definition.yaml`.
fn definition_path(repo: &str) -> PathBuf {
    let p = Path::new(repo);
    if p.is_file() {
        p.to_path_buf()
    } else {
        p.join("definition.yaml")
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/definition", get(get_definition))
        .route("/api/profiles/{profile}/layers", get(get_layers))
        .route("/api/profiles/{profile}/render", get(get_render))
        .route("/api/profiles/{profile}/evidence", get(get_evidence))
        .route("/api/access", get(get_access))
        .fallback(get(serve_ui))
        .with_state(state)
}

/// A JSON error body — the wire shape the UI's protocol package mirrors.
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, axum::Json(json!({ "error": self.1 }))).into_response()
    }
}

#[derive(Serialize)]
struct DefinitionView {
    name: String,
    description: Option<String>,
    profiles: Vec<ProfileView>,
    /// The level vocabulary and scope tree. The UI's context spine is built from this, so no level
    /// name is ever hardcoded client-side — a scope's level is the part of its id before the slash.
    hierarchy: HierarchyView,
    nodes: Vec<NodeView>,
}

#[derive(Serialize)]
struct HierarchyView {
    levels: Vec<String>,
    scopes: Vec<ScopeView>,
}

#[derive(Serialize)]
struct ScopeView {
    id: String,
    parent: Option<String>,
    layer: Option<String>,
}

#[derive(Serialize)]
struct ProfileView {
    name: String,
    family: String,
}

#[derive(Serialize)]
struct NodeView {
    key: String,
    scope: String,
    components: Vec<ComponentView>,
}

#[derive(Serialize)]
struct ComponentView {
    name: String,
    /// The component's config leaf — the last entry in its merge chain.
    layer: Option<String>,
}

/// The definition's shape: metadata, the profiles it declares, and the shared topology.
async fn get_definition(State(state): State<Arc<AppState>>) -> Response {
    let a = &state.loaded.authored;
    let view = DefinitionView {
        name: a.metadata.name.clone(),
        description: a.metadata.description.clone(),
        profiles: a
            .profiles
            .iter()
            .map(|(name, p)| ProfileView {
                name: name.clone(),
                family: p.family.clone(),
            })
            .collect(),
        hierarchy: HierarchyView {
            levels: a.hierarchy.levels.clone(),
            scopes: a
                .hierarchy
                .scopes
                .iter()
                .map(|s| ScopeView {
                    id: s.id.clone(),
                    parent: s.parent.clone(),
                    layer: s.layer.clone(),
                })
                .collect(),
        },
        nodes: a
            .topology
            .nodes
            .iter()
            .map(|n| NodeView {
                key: n.key.clone(),
                scope: n.scope.clone(),
                components: n
                    .components
                    .iter()
                    .map(|c| ComponentView {
                        name: c.name.clone(),
                        layer: c.layer.clone(),
                    })
                    .collect(),
            })
            .collect(),
    };
    axum::Json(view).into_response()
}

/// The config-layers screen's data: the effective (merged) config per node × component, for every
/// environment a profile declares.
async fn get_layers(
    State(state): State<Arc<AppState>>,
    AxumPath(profile): AxumPath<String>,
) -> Response {
    let ws = match state.loaded.workspace(&profile) {
        Ok(ws) => ws,
        Err(e) => return ApiError(StatusCode::NOT_FOUND, e).into_response(),
    };
    let mut environments = Vec::new();
    for env in &ws.definition.environments {
        match ec_deploy::render::effective_configs(&ws, &env.name) {
            Ok(configs) => {
                let items: Vec<Value> = configs
                    .into_iter()
                    .map(|(node, component, config)| {
                        json!({ "node": node, "component": component, "config": config })
                    })
                    .collect();
                environments.push(json!({ "environment": env.name, "components": items }));
            }
            Err(e) => {
                return ApiError(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response();
            }
        }
    }
    axum::Json(json!({ "profile": profile, "environments": environments })).into_response()
}

/// The render-review screen's data: the rendered artifacts + the normalized plan for a profile,
/// rendered against its platform family for its first environment.
async fn get_render(
    State(state): State<Arc<AppState>>,
    AxumPath(profile): AxumPath<String>,
) -> Response {
    let ws = match state.loaded.workspace(&profile) {
        Ok(ws) => ws,
        Err(e) => return ApiError(StatusCode::NOT_FOUND, e).into_response(),
    };
    let Some(target) = Platform::from_family(&ws.definition.target_standard.family) else {
        return ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown family {}", ws.definition.target_standard.family),
        )
        .into_response();
    };
    let Some(env) = ws.definition.environments.first() else {
        return ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "profile declares no environment".into(),
        )
        .into_response();
    };
    match render(&ws, &env.name, target, "initial") {
        Ok(output) => {
            let files: Vec<Value> = output
                .files
                .iter()
                .map(|f| json!({ "path": f.path, "text": f.text }))
                .collect();
            axum::Json(json!({
                "profile": profile,
                "target": ws.definition.target_standard.family,
                "environment": env.name,
                "files": files,
                "plan": output.plan,
            }))
            .into_response()
        }
        Err(e) => ApiError(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// The evidence-correlation screen's data: the release lock a profile would produce — the
/// correlation envelope over the two streams (config and artifact, correlated but never fused),
/// per-file `sha256`, the `devMode` flag, and the evidence bundle (schema + semantic checks,
/// warnings, render determinism). This is the Studio adjudicating delivery **from evidence** while
/// holding intent (REVIEW #13); it computes the envelope in-memory and writes nothing.
async fn get_evidence(
    State(state): State<Arc<AppState>>,
    AxumPath(profile): AxumPath<String>,
) -> Response {
    let ws = match state.loaded.workspace(&profile) {
        Ok(ws) => ws,
        Err(e) => return ApiError(StatusCode::NOT_FOUND, e).into_response(),
    };
    let Some(target) = Platform::from_family(&ws.definition.target_standard.family) else {
        return ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown family {}", ws.definition.target_standard.family),
        )
        .into_response();
    };
    let Some(env) = ws.definition.environments.first().map(|e| e.name.clone()) else {
        return ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "profile declares no environment".into(),
        )
        .into_response();
    };
    // A release is only honest over a valid definition; run the same semantic gate the CLI does and
    // refuse to fabricate an envelope for a broken plant.
    let findings = kernel_validate(&ws, None);
    if !findings.errors.is_empty() {
        return ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "the definition has {} semantic error(s); evidence is only produced for a valid \
                 definition (run `deployment validate`)",
                findings.errors.len()
            ),
        )
        .into_response();
    }
    let commit = ec_adapters::describe_head(&state.loaded.root).unwrap_or_else(|| "unknown".into());
    // The envelope correlates both streams regardless of which is promoted; we build once and label
    // both independent release tags.
    let output = match build_release(
        &ws,
        &env,
        target,
        Stream::Config,
        "initial",
        &commit,
        &findings.warnings,
        0,
    ) {
        Ok(o) => o,
        Err(e) => return ApiError(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    };
    let read = |name: &str| -> Value {
        output
            .files
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, text)| serde_json::from_str(text).ok())
            .unwrap_or(Value::Null)
    };
    let short = commit.get(..12).unwrap_or(&commit);
    axum::Json(json!({
        "profile": profile,
        "target": ws.definition.target_standard.family,
        "environment": env,
        "commit": commit,
        "streamTags": { "config": format!("config-{short}"), "artifact": format!("artifact-{short}") },
        "manifest": read("manifest.json"),
        "evidence": read("evidence.json"),
    }))
    .into_response()
}

/// The access-control screen's data: a rendering of the repository's `CODEOWNERS` (REVIEW #10). For
/// the definition file and each component's config layer, it reports who a change would require as a
/// reviewer — never inventing an approval lane, only surfacing the Git-host rule that already
/// governs the file. A repository with no `CODEOWNERS` is reported honestly as falling to the
/// default branch-protection review.
async fn get_access(State(state): State<Arc<AppState>>) -> Response {
    let root = &state.loaded.root;
    let codeowners = ec_adapters::read_codeowners(root);
    let (owners_path, parsed) = match &codeowners {
        Some((path, text)) => (Some(path.clone()), CodeOwners::parse(text)),
        None => (None, CodeOwners::default()),
    };

    // Resolve one file (given as an absolute path) to (repo-relative path, matched pattern, owners).
    let resolve = |abs: &Path| -> Value {
        let repo_rel = ec_adapters::repo_relative(root, abs);
        match parsed.owner_of(&repo_rel) {
            Some(m) => json!({
                "file": repo_rel,
                "owners": m.owners,
                "matchedPattern": m.pattern,
            }),
            None => json!({ "file": repo_rel, "owners": [], "matchedPattern": Value::Null }),
        }
    };

    let a = &state.loaded.authored;
    let definition_file = resolve(&state.loaded.definition_path);

    let mut items = Vec::new();
    for node in &a.topology.nodes {
        for comp in &node.components {
            if let Some(layer) = &comp.layer {
                let mut entry = resolve(&root.join(layer));
                if let Value::Object(map) = &mut entry {
                    map.insert("node".into(), json!(node.key));
                    map.insert("scope".into(), json!(node.scope));
                    map.insert("component".into(), json!(comp.name));
                }
                items.push(entry);
            }
        }
    }

    let is_unowned = |i: &Value| -> bool { i["owners"].as_array().is_none_or(Vec::is_empty) };
    let unowned =
        items.iter().filter(|i| is_unowned(i)).count() + usize::from(is_unowned(&definition_file));

    let note = access_note(owners_path.as_deref(), unowned);
    axum::Json(json!({
        "codeowners": owners_path.map(|p| json!({ "path": p })).unwrap_or(Value::Null),
        "unownedCount": unowned,
        "note": note,
        "definitionFile": definition_file,
        "items": items,
    }))
    .into_response()
}

/// The honest one-line summary the UI shows above the table.
fn access_note(codeowners_path: Option<&str>, unowned: usize) -> String {
    match codeowners_path {
        None => "This repository defines no CODEOWNERS; changes to every file fall to the default \
                 branch-protection review."
            .to_string(),
        Some(path) => {
            if unowned == 0 {
                format!(
                    "Every deployment file is owned by a {path} rule; a change requires its owners' review."
                )
            } else {
                format!(
                    "{unowned} deployment file(s) match no {path} rule and fall to the default \
                     branch-protection review; the rest require their owners' review."
                )
            }
        }
    }
}

/// Serve the embedded SPA: an exact asset path returns that file with its content type; anything
/// else returns `index.html` so the client-side router can take over (single-page-app fallback).
async fn serve_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = UI.get_file(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            [(header::CONTENT_TYPE, mime.as_ref())],
            file.contents().to_vec(),
        )
            .into_response();
    }
    match UI.get_file("index.html") {
        Some(index) => Html(index.contents().to_vec()).into_response(),
        None => (StatusCode::NOT_FOUND, "UI bundle not built").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const DEF: &str = r#"
apiVersion: edgecommons.io/v1alpha1
kind: DeploymentDefinition
metadata: { name: studio-demo, description: a tiny plant }
hierarchy:
  levels: [site, device]
  scopes:
    - { id: site/lab, parent: null }
topology:
  nodes:
    - key: box-01
      scope: site/lab
      components:
        - name: telemetry-processor
          layer: layers/telemetry.json
profiles:
  host:
    family: HOST
    environments: [{ name: local, bindings: bindings/local.json }]
    defaults: { configSource: FILE }
    nodes:
      box-01:
        components:
          telemetry-processor:
            artifact: { source: { kind: sibling, repo: telemetry-processor } }
            launch: { order: 30 }
"#;

    fn fixture() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bindings")).unwrap();
        std::fs::write(dir.path().join("bindings/local.json"), "{}\n").unwrap();
        std::fs::create_dir_all(dir.path().join("layers")).unwrap();
        std::fs::write(
            dir.path().join("layers/telemetry.json"),
            r#"{ "component": { "global": { "publishIntervalMs": 500 } } }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("definition.yaml"), DEF).unwrap();
        let loaded = ec_adapters::load_workspace(&dir.path().join("definition.yaml")).unwrap();
        (dir, Arc::new(AppState { loaded }))
    }

    async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, body)
    }

    #[tokio::test]
    async fn definition_endpoint_reports_the_plant() {
        let (_d, state) = fixture();
        let app = router(state);
        let (status, body) = get(&app, "/api/definition").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "studio-demo");
        assert_eq!(body["profiles"][0]["family"], "HOST");
        assert_eq!(body["nodes"][0]["key"], "box-01");
        assert_eq!(
            body["nodes"][0]["components"][0]["name"],
            "telemetry-processor"
        );
        assert_eq!(
            body["nodes"][0]["components"][0]["layer"],
            "layers/telemetry.json"
        );
        // The context spine is built from this: the level vocabulary and the scope tree, so the
        // UI never hardcodes a level name.
        assert_eq!(body["hierarchy"]["levels"][0], "site");
        assert_eq!(body["hierarchy"]["scopes"][0]["id"], "site/lab");
        assert_eq!(body["hierarchy"]["scopes"][0]["parent"], Value::Null);
    }

    #[tokio::test]
    async fn layers_endpoint_returns_the_effective_config_per_component() {
        let (_d, state) = fixture();
        let app = router(state);
        let (status, body) = get(&app, "/api/profiles/host/layers").await;
        assert_eq!(status, StatusCode::OK);
        let comps = &body["environments"][0]["components"];
        assert_eq!(comps[0]["component"], "telemetry-processor");
        // The effective config is the placement-derived merge (identity stamped from the scope).
        assert_eq!(comps[0]["config"]["identity"]["site"], "lab");
    }

    #[tokio::test]
    async fn render_endpoint_returns_files_and_plan() {
        let (_d, state) = fixture();
        let app = router(state);
        let (status, body) = get(&app, "/api/profiles/host/render").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["target"], "HOST");
        assert!(
            body["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["path"].as_str().unwrap().contains("supervisord.conf"))
        );
        assert!(!body["plan"]["entries"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unknown_profile_is_a_404() {
        let (_d, state) = fixture();
        let app = router(state);
        let (status, body) = get(&app, "/api/profiles/openshift/layers").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("openshift"));
    }

    #[tokio::test]
    async fn evidence_endpoint_returns_the_correlation_envelope() {
        let (_d, state) = fixture();
        let app = router(state);
        let (status, body) = get(&app, "/api/profiles/host/evidence").await;
        assert_eq!(status, StatusCode::OK);
        // Both streams are present and correlated, never fused.
        assert!(
            !body["manifest"]["streams"]["artifact"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // The fixture's artifact is source-form (no version+digest), so the envelope is devMode.
        assert_eq!(body["manifest"]["devMode"], true);
        // Evidence records the semantic gate and its two independent release tags.
        assert!(body["evidence"]["semanticRules"].is_string());
        assert!(
            body["streamTags"]["config"]
                .as_str()
                .unwrap()
                .starts_with("config-")
        );
        assert!(
            body["streamTags"]["artifact"]
                .as_str()
                .unwrap()
                .starts_with("artifact-")
        );
    }

    #[tokio::test]
    async fn access_endpoint_degrades_honestly_without_codeowners() {
        // The tempdir is not a Git repository, so there is no CODEOWNERS to render.
        let (_d, state) = fixture();
        let app = router(state);
        let (status, body) = get(&app, "/api/access").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["codeowners"], Value::Null);
        assert!(body["note"].as_str().unwrap().contains("branch-protection"));
        // Every file falls through to default review; none carries owners.
        assert!(body["items"][0]["owners"].as_array().unwrap().is_empty());
        assert_eq!(body["items"][0]["component"], "telemetry-processor");
    }

    #[tokio::test]
    async fn access_endpoint_renders_codeowners_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bindings")).unwrap();
        std::fs::write(dir.path().join("bindings/local.json"), "{}\n").unwrap();
        std::fs::create_dir_all(dir.path().join("layers")).unwrap();
        std::fs::write(
            dir.path().join("layers/telemetry.json"),
            r#"{ "component": { "global": { "publishIntervalMs": 500 } } }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("definition.yaml"), DEF).unwrap();
        // A catch-all plus a leaf rule (last-match-wins). `git init` is enough for
        // `rev-parse --show-toplevel`; no commit is needed for ownership resolution.
        std::fs::write(
            dir.path().join("CODEOWNERS"),
            "* @plant-eng\ntelemetry.json @telemetry-team\n",
        )
        .unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .arg("init")
                .output()
                .is_ok_and(|o| o.status.success()),
            "git init must succeed for the CODEOWNERS test"
        );
        let loaded = ec_adapters::load_workspace(&dir.path().join("definition.yaml")).unwrap();
        let app = router(Arc::new(AppState { loaded }));
        let (status, body) = get(&app, "/api/access").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["codeowners"]["path"], "CODEOWNERS");
        // The leaf rule wins for the telemetry layer; the definition falls to the catch-all.
        assert_eq!(body["items"][0]["owners"][0], "@telemetry-team");
        assert_eq!(body["definitionFile"]["owners"][0], "@plant-eng");
        assert_eq!(body["unownedCount"], 0);
    }

    #[tokio::test]
    async fn the_index_serves_the_read_only_shell() {
        let (_d, state) = fixture();
        let app = router(state);
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("Deployment Studio"));
    }

    #[test]
    fn definition_path_resolves_a_dir_or_a_file() {
        assert_eq!(
            definition_path("/repo").file_name().unwrap(),
            "definition.yaml"
        );
    }
}
