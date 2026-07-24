//! The Deployment Studio server (DESIGN-cli §8.4, deck ch. 12): an `axum` service over the **same
//! kernel the CLI uses**. The server adds branch/draft orchestration, the UI, evidence correlation,
//! and access control — never a capability the CLI lacks. Git is the only durable state.
//!
//! This is the read-only first cut (deck ch. 13 slice 5): it loads a definition from the repo and
//! serves the config layers and the render/plan for any profile. No authoring, no writes, no cloud
//! SDK above the port boundary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use ec_deploy::Platform;
use ec_deploy::render::render;
use ec_diag::Fatal;
use serde::Serialize;
use serde_json::{Value, json};

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
        .fallback(get(index))
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
    nodes: Vec<NodeView>,
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
    components: Vec<String>,
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
        nodes: a
            .topology
            .nodes
            .iter()
            .map(|n| NodeView {
                key: n.key.clone(),
                scope: n.scope.clone(),
                components: n.components.iter().map(|c| c.name.clone()).collect(),
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

/// The SPA entry point. The React + Carbon bundle is embedded here once built (deck ch. 12); until
/// then this is a read-only status page confirming the server and its API are live.
async fn index() -> Html<&'static str> {
    Html(include_str!("index.html"))
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
        assert_eq!(body["nodes"][0]["components"][0], "telemetry-processor");
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
