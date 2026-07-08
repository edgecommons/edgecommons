//! # Configuration — split/layered config
//!
//! Internal split-config coordinator for the Rust core library. It keeps the public config surface
//! unchanged: callers still see only the stripped, merged effective config snapshot.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, recommended_watcher};
use serde_json::{Map, Value};
use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::cli::ConfigSourceSpec;
use crate::error::{EdgeCommonsError, Result};

use super::source::ConfigSource;

const SHARED_CONFIG_ENV: &str = "EDGECOMMONS_SHARED_CONFIG";
#[cfg(feature = "greengrass")]
const SHARED_COMPONENT_ENV: &str = "EDGECOMMONS_SHARED_COMPONENT";
#[cfg(feature = "greengrass")]
const DEFAULT_SHARED_COMPONENT: &str = "com.mbreissi.edgecommons.EdgeCommonsSharedConfig";
#[cfg(feature = "greengrass")]
const SHARED_GG_CONFIG_KEY: &str = "SharedComponentConfig";
#[cfg(feature = "greengrass")]
const SHARED_SHADOW_NAME: &str = "edgecommons-shared";

/// A config source wrapper that returns effective split-config documents.
pub struct LayeredConfigSource {
    component: Arc<dyn ConfigSource>,
    spec: ConfigSourceSpec,
    no_shared_config: bool,
    thing_name: String,
    latest_component: Arc<Mutex<Option<Value>>>,
    latest_base: Arc<Mutex<Option<Value>>>,
}

impl LayeredConfigSource {
    /// Wrap an existing component-layer source with split-config merge behavior.
    pub fn new(
        component: Arc<dyn ConfigSource>,
        spec: ConfigSourceSpec,
        no_shared_config: bool,
        thing_name: impl Into<String>,
    ) -> Self {
        Self {
            component,
            spec,
            no_shared_config,
            thing_name: thing_name.into(),
            latest_component: Arc::new(Mutex::new(None)),
            latest_base: Arc::new(Mutex::new(None)),
        }
    }

    async fn load_effective(&self) -> Result<Value> {
        let raw = self.component.load().await?;
        self.apply_payload(raw, false).await
    }

    async fn apply_payload(&self, raw: Value, preserve_legacy_base: bool) -> Result<Value> {
        let legacy_base =
            if preserve_legacy_base && matches!(self.spec, ConfigSourceSpec::ConfigComponent) {
                self.latest_base.lock().ok().and_then(|base| base.clone())
            } else {
                None
            };
        let candidate = effective_candidate_from_source_payload(
            &self.spec,
            raw,
            self.no_shared_config,
            &self.thing_name,
            legacy_base,
        )
        .await?;
        if let Ok(mut latest) = self.latest_component.lock() {
            *latest = Some(candidate.component.clone());
        }
        if let Ok(mut latest) = self.latest_base.lock() {
            *latest = candidate.base.clone();
        }
        Ok(candidate.effective)
    }
}

#[async_trait]
impl ConfigSource for LayeredConfigSource {
    async fn load(&self) -> Result<Value> {
        self.load_effective().await
    }

    fn source_name(&self) -> &str {
        self.component.source_name()
    }

    fn watch(&self) -> Option<UnboundedReceiver<Value>> {
        let component_rx = self.component.watch();
        let base_watch = self
            .latest_component
            .lock()
            .ok()
            .and_then(|latest| {
                latest.as_ref().and_then(|c| {
                    base_watch_target(&self.spec, c, self.no_shared_config, &self.thing_name)
                        .ok()
                        .flatten()
                })
            })
            .and_then(watch_base);

        if component_rx.is_none() && base_watch.is_none() {
            return None;
        }

        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let source = Arc::new(Self {
            component: self.component.clone(),
            spec: self.spec.clone(),
            no_shared_config: self.no_shared_config,
            thing_name: self.thing_name.clone(),
            latest_component: self.latest_component.clone(),
            latest_base: self.latest_base.clone(),
        });

        tokio::spawn(async move {
            let mut component_rx = component_rx;
            let (mut base_rx, mut _base_watcher) = match base_watch {
                Some(w) => (Some(w.rx), Some(w.guard)),
                None => (None, None),
            };
            loop {
                tokio::select! {
                    update = async {
                        match &mut component_rx {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if update.is_none() {
                            component_rx = None;
                            if base_rx.is_none() {
                                break;
                            }
                            continue;
                        }
                        let raw = update.expect("checked above");
                        if let Err(e) = apply_payload_and_forward(&source, raw, true, &out_tx).await {
                            tracing::warn!(error = %e, "split-config component reload failed; keeping previous");
                        }
                        if let Some(next) = source.latest_component.lock().ok().and_then(|latest| latest.as_ref().and_then(|c| base_watch_target(&source.spec, c, source.no_shared_config, &source.thing_name).ok().flatten())).and_then(watch_base) {
                            base_rx = Some(next.rx);
                            _base_watcher = Some(next.guard);
                        }
                    }
                    changed = async {
                        match &mut base_rx {
                            Some(rx) => rx.recv().await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if changed.is_none() {
                            base_rx = None;
                            _base_watcher = None;
                            if component_rx.is_none() {
                                break;
                            }
                            continue;
                        }
                        if let Err(e) = reload_and_forward(&source, &out_tx).await {
                            tracing::warn!(error = %e, "split-config shared-layer reload failed; keeping previous");
                        }
                    }
                }
            }
        });

        Some(out_rx)
    }
}

async fn reload_and_forward(
    source: &LayeredConfigSource,
    out_tx: &mpsc::UnboundedSender<Value>,
) -> Result<()> {
    let effective = source.load_effective().await?;
    let _ = out_tx.send(effective);
    Ok(())
}

async fn apply_payload_and_forward(
    source: &LayeredConfigSource,
    raw: Value,
    preserve_legacy_base: bool,
    out_tx: &mpsc::UnboundedSender<Value>,
) -> Result<()> {
    let effective = source.apply_payload(raw, preserve_legacy_base).await?;
    let _ = out_tx.send(effective);
    Ok(())
}

/// Merge layers using the split-config deep-merge rules. This pure function strips raw control
/// fields but does not apply coordinator-only checks such as rejecting `extends` in a resolved base.
#[must_use]
pub fn deep_merge(layers: &[Value]) -> Value {
    let mut result = Value::Object(Map::new());
    for layer in layers {
        result = merge_value(result, strip_controls(layer), "$");
    }
    result
}

fn merge_value(left: Value, right: Value, path: &str) -> Value {
    match (left, right) {
        (Value::Object(mut l), Value::Object(r)) => {
            for (key, value) in r {
                let child_path = if path == "$" {
                    format!("$.{key}")
                } else {
                    format!("{path}.{key}")
                };
                match l.remove(&key) {
                    Some(existing) => {
                        l.insert(key, merge_value(existing, value, &child_path));
                    }
                    None => {
                        l.insert(key, value);
                    }
                }
            }
            Value::Object(l)
        }
        (left, right) => {
            if type_conflict_should_warn(&left, &right) {
                tracing::warn!(path, "split-config type conflict; later layer wins");
            }
            right
        }
    }
}

fn type_conflict_should_warn(left: &Value, right: &Value) -> bool {
    !left.is_null()
        && !right.is_null()
        && !left.is_array()
        && !right.is_array()
        && value_kind(left) != value_kind(right)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn strip_controls(layer: &Value) -> Value {
    match layer {
        Value::Object(obj) => {
            let mut out = obj.clone();
            out.remove("extends");
            out.remove("sharedConfig");
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
async fn effective_from_source_payload(
    spec: &ConfigSourceSpec,
    payload: Value,
    no_shared_config: bool,
    thing_name: &str,
) -> Result<Value> {
    effective_candidate_from_source_payload(spec, payload, no_shared_config, thing_name, None)
        .await
        .map(|candidate| candidate.effective)
}

async fn effective_candidate_from_source_payload(
    spec: &ConfigSourceSpec,
    payload: Value,
    no_shared_config: bool,
    thing_name: &str,
    legacy_base: Option<Value>,
) -> Result<EffectiveLayers> {
    let parsed = parse_source_payload(spec, payload)?;
    if matches!(spec, ConfigSourceSpec::ConfigComponent) {
        let mut bundle = parsed.bundle;
        if !parsed.base_present {
            bundle.base = legacy_base;
        }
        return merge_bundle_candidate(bundle, no_shared_config);
    }
    let base = resolve_base(spec, &parsed.bundle.component, no_shared_config, thing_name).await?;
    merge_bundle_candidate(
        LayerBundle {
            base,
            component: parsed.bundle.component,
        },
        no_shared_config,
    )
}

fn parse_source_payload(spec: &ConfigSourceSpec, payload: Value) -> Result<ParsedLayerBundle> {
    if matches!(spec, ConfigSourceSpec::ConfigComponent) {
        parse_config_component_payload(payload)
    } else {
        ensure_object(payload, "component layer").map(|component| ParsedLayerBundle {
            bundle: LayerBundle {
                base: None,
                component,
            },
            base_present: false,
        })
    }
}

fn parse_config_component_payload(payload: Value) -> Result<ParsedLayerBundle> {
    if structured_error(&payload).is_some() {
        let (code, message) = structured_error(&payload).expect("checked above");
        return Err(EdgeCommonsError::Config(format!(
            "CONFIG_COMPONENT server error {code}: {message}"
        )));
    }
    let Some(obj) = payload.as_object() else {
        return Err(EdgeCommonsError::Config(
            "CONFIG_COMPONENT payload must be a JSON object".to_string(),
        ));
    };
    if !obj.contains_key("base") {
        return Ok(ParsedLayerBundle {
            bundle: LayerBundle {
                base: None,
                component: payload,
            },
            base_present: false,
        });
    }

    let component = obj.get("component").ok_or_else(|| {
        EdgeCommonsError::Config("CONFIG_COMPONENT_BUNDLE_INVALID: missing component".to_string())
    })?;
    let component =
        ensure_object(component.clone(), "CONFIG_COMPONENT bundle component").map_err(|_| {
            EdgeCommonsError::Config(
                "CONFIG_COMPONENT_BUNDLE_INVALID: component must be an object".to_string(),
            )
        })?;
    let base = match obj.get("base") {
        Some(Value::Null) => None,
        Some(value) => Some(
            ensure_object(value.clone(), "CONFIG_COMPONENT bundle base").map_err(|_| {
                EdgeCommonsError::Config(
                    "CONFIG_COMPONENT_BUNDLE_INVALID: base must be object or null".to_string(),
                )
            })?,
        ),
        None => None,
    };
    Ok(ParsedLayerBundle {
        bundle: LayerBundle { base, component },
        base_present: true,
    })
}

fn structured_error(payload: &Value) -> Option<(String, String)> {
    let obj = payload.as_object()?;
    if obj.get("ok").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let err = obj.get("error")?.as_object()?;
    let code = err.get("code")?.as_str()?.to_string();
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((code, message))
}

#[cfg(test)]
fn merge_bundle(bundle: LayerBundle, no_shared_config: bool) -> Result<Value> {
    merge_bundle_candidate(bundle, no_shared_config).map(|candidate| candidate.effective)
}

fn merge_bundle_candidate(bundle: LayerBundle, no_shared_config: bool) -> Result<EffectiveLayers> {
    validate_shared_config_control(&bundle.component)?;
    let enabled = shared_config_enabled(&bundle.component, no_shared_config)?;
    if !enabled {
        tracing::info!("split-config shared layer disabled");
        let effective = deep_merge(std::slice::from_ref(&bundle.component));
        return Ok(EffectiveLayers {
            effective,
            component: bundle.component,
            base: None,
        });
    }
    match bundle.base {
        Some(base) => {
            reject_base_extends(&base)?;
            tracing::info!("split-config shared layer applied");
            let effective = deep_merge(&[base.clone(), bundle.component.clone()]);
            Ok(EffectiveLayers {
                effective,
                component: bundle.component,
                base: Some(base),
            })
        }
        None => {
            tracing::info!("split-config shared layer absent");
            let effective = deep_merge(std::slice::from_ref(&bundle.component));
            Ok(EffectiveLayers {
                effective,
                component: bundle.component,
                base: None,
            })
        }
    }
}

#[derive(Debug)]
struct LayerBundle {
    base: Option<Value>,
    component: Value,
}

#[derive(Debug)]
struct ParsedLayerBundle {
    bundle: LayerBundle,
    base_present: bool,
}

#[derive(Debug)]
struct EffectiveLayers {
    effective: Value,
    component: Value,
    base: Option<Value>,
}

async fn resolve_base(
    spec: &ConfigSourceSpec,
    component: &Value,
    no_shared_config: bool,
    _thing_name: &str,
) -> Result<Option<Value>> {
    if !shared_config_enabled(component, no_shared_config)? {
        return Ok(None);
    }
    match spec {
        ConfigSourceSpec::File { path } => resolve_file_base(component, path, false).await,
        ConfigSourceSpec::ConfigMap { mount_dir, key } => {
            let dir = mount_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(super::source::configmap::DEFAULT_MOUNT_DIR));
            let file = dir.join(
                key.clone()
                    .unwrap_or_else(|| super::source::configmap::DEFAULT_KEY.to_string()),
            );
            resolve_file_base(component, &file, true).await
        }
        ConfigSourceSpec::Env { .. } => resolve_env_base().await,
        ConfigSourceSpec::ConfigComponent => Ok(None),
        #[cfg(feature = "greengrass")]
        ConfigSourceSpec::Greengrass { .. } => resolve_greengrass_base().await,
        #[cfg(feature = "greengrass")]
        ConfigSourceSpec::Shadow { .. } => resolve_shadow_base(_thing_name).await,
        #[cfg(not(feature = "greengrass"))]
        ConfigSourceSpec::Greengrass { .. } | ConfigSourceSpec::Shadow { .. } => Ok(None),
    }
}

async fn resolve_file_base(
    component: &Value,
    component_path: &Path,
    configmap: bool,
) -> Result<Option<Value>> {
    let candidate = base_path_for_file_family(component, component_path, configmap)?;
    let Some(candidate) = candidate else {
        return Ok(None);
    };
    match tokio::fs::read(&candidate.path).await {
        Ok(bytes) => parse_base_bytes(&bytes, &candidate.path),
        Err(e) if candidate.missing_is_noop && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(EdgeCommonsError::Config(format!(
            "failed to read shared config '{}': {e}",
            candidate.path.display()
        ))),
    }
}

async fn resolve_env_base() -> Result<Option<Value>> {
    let raw = match std::env::var(SHARED_CONFIG_ENV) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if let Some(path) = raw.strip_prefix('@') {
        if path.is_empty() {
            return Err(EdgeCommonsError::Config(
                "EDGECOMMONS_SHARED_CONFIG @path must not be empty".to_string(),
            ));
        }
        let path = PathBuf::from(path);
        let bytes = tokio::fs::read(&path).await.map_err(|e| {
            EdgeCommonsError::Config(format!(
                "failed to read shared config '{}': {e}",
                path.display()
            ))
        })?;
        parse_base_bytes(&bytes, &path)
    } else {
        parse_base_value(serde_json::from_str(&raw)?, "EDGECOMMONS_SHARED_CONFIG")
    }
}

#[cfg(feature = "greengrass")]
async fn resolve_greengrass_base() -> Result<Option<Value>> {
    let explicit_component = std::env::var(SHARED_COMPONENT_ENV).ok();
    let component = explicit_component
        .clone()
        .unwrap_or_else(|| DEFAULT_SHARED_COMPONENT.to_string());
    let source = super::source::greengrass::GreengrassConfigSource::new(
        Some(component.clone()),
        SHARED_GG_CONFIG_KEY.to_string(),
    );
    match source.load().await {
        Ok(value) => parse_base_value(value, &format!("{component}/{SHARED_GG_CONFIG_KEY}")),
        Err(_e) if explicit_component.is_none() => {
            tracing::info!(
                component,
                key = SHARED_GG_CONFIG_KEY,
                "default Greengrass shared config is absent"
            );
            Ok(None)
        }
        Err(e) => Err(EdgeCommonsError::Config(format!(
            "SHARED_CONFIG_UNAVAILABLE: {e}"
        ))),
    }
}

#[cfg(feature = "greengrass")]
async fn resolve_shadow_base(thing_name: &str) -> Result<Option<Value>> {
    use crate::ipc;

    let rt = ipc::global();
    rt.connect().await?;
    let bytes = match rt
        .get_shadow(thing_name, Some(SHARED_SHADOW_NAME.to_string()))
        .await
    {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => return Ok(None),
    };
    let doc: Value = serde_json::from_slice(&bytes)?;
    let config_str = extract_shadow_component_config(&doc).ok_or_else(|| {
        EdgeCommonsError::Config(
            "shared shadow exists but has no string ComponentConfig".to_string(),
        )
    })?;
    parse_base_value(serde_json::from_str(&config_str)?, SHARED_SHADOW_NAME)
}

#[cfg(feature = "greengrass")]
fn extract_shadow_component_config(doc: &Value) -> Option<String> {
    let state = doc.get("state")?;
    for key in ["desired", "reported"] {
        if let Some(s) = state
            .get(key)
            .and_then(|d| d.get("ComponentConfig"))
            .and_then(Value::as_str)
        {
            return Some(s.to_string());
        }
    }
    None
}

fn parse_base_bytes(bytes: &[u8], source: &Path) -> Result<Option<Value>> {
    parse_base_value(
        serde_json::from_slice(bytes)?,
        &source.display().to_string(),
    )
}

fn parse_base_value(value: Value, source: &str) -> Result<Option<Value>> {
    let value = ensure_object(value, "shared config").map_err(|_| {
        EdgeCommonsError::Config(format!("shared config '{source}' must be a JSON object"))
    })?;
    reject_base_extends(&value)?;
    Ok(Some(value))
}

fn ensure_object(value: Value, label: &str) -> Result<Value> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(EdgeCommonsError::Config(format!(
            "{label} must be a JSON object"
        )))
    }
}

fn validate_shared_config_control(component: &Value) -> Result<()> {
    if let Some(value) = component.get("sharedConfig") {
        if !value.is_boolean() {
            return Err(EdgeCommonsError::Config(
                "sharedConfig must be a boolean when present".to_string(),
            ));
        }
    }
    if let Some(value) = component.get("extends") {
        if value.as_str().is_none_or(|value| value.is_empty()) {
            return Err(EdgeCommonsError::Config(
                "extends must be a non-empty string when present".to_string(),
            ));
        }
    }
    Ok(())
}

fn shared_config_enabled(component: &Value, no_shared_config: bool) -> Result<bool> {
    validate_shared_config_control(component)?;
    if no_shared_config {
        return Ok(false);
    }
    Ok(component
        .get("sharedConfig")
        .and_then(Value::as_bool)
        .unwrap_or(true))
}

fn reject_base_extends(base: &Value) -> Result<()> {
    if base.get("extends").is_some() {
        return Err(EdgeCommonsError::Config(
            "N-layer inheritance not implemented: shared config must not contain extends"
                .to_string(),
        ));
    }
    Ok(())
}

fn base_watch_target(
    spec: &ConfigSourceSpec,
    component: &Value,
    no_shared_config: bool,
    _thing_name: &str,
) -> Result<Option<BaseWatchTarget>> {
    if !shared_config_enabled(component, no_shared_config)? {
        return Ok(None);
    }
    match spec {
        ConfigSourceSpec::File { path } => Ok(base_path_for_file_family(component, path, false)?
            .filter(|candidate| candidate.path.exists())
            .map(|candidate| BaseWatchTarget::File(candidate.path))),
        ConfigSourceSpec::ConfigMap { mount_dir, key } => {
            let dir = mount_dir
                .clone()
                .unwrap_or_else(|| PathBuf::from(super::source::configmap::DEFAULT_MOUNT_DIR));
            let file = dir.join(
                key.clone()
                    .unwrap_or_else(|| super::source::configmap::DEFAULT_KEY.to_string()),
            );
            Ok(base_path_for_file_family(component, &file, true)?
                .filter(|candidate| candidate.path.exists())
                .map(|candidate| BaseWatchTarget::ConfigMap(candidate.path)))
        }
        #[cfg(feature = "greengrass")]
        ConfigSourceSpec::Shadow { .. } => Ok(Some(BaseWatchTarget::Shadow {
            thing_name: _thing_name.to_string(),
        })),
        _ => Ok(None),
    }
}

struct BasePathCandidate {
    path: PathBuf,
    missing_is_noop: bool,
}

fn base_path_for_file_family(
    component: &Value,
    component_path: &Path,
    configmap: bool,
) -> Result<Option<BasePathCandidate>> {
    if let Some(value) = component.get("extends") {
        let Some(raw) = value.as_str().filter(|s| !s.is_empty()) else {
            return Err(EdgeCommonsError::Config(
                "extends must be a non-empty string".to_string(),
            ));
        };
        let path = PathBuf::from(raw);
        let path = if path.is_absolute() {
            path
        } else {
            component_path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .join(path)
        };
        validate_configmap_base_key(&path, configmap)?;
        return Ok(Some(BasePathCandidate {
            path,
            missing_is_noop: false,
        }));
    }

    if let Ok(raw) = std::env::var(SHARED_CONFIG_ENV) {
        if raw.is_empty() {
            return Err(EdgeCommonsError::Config(format!(
                "{SHARED_CONFIG_ENV} must not be empty"
            )));
        }
        let path = PathBuf::from(raw);
        validate_configmap_base_key(&path, configmap)?;
        return Ok(Some(BasePathCandidate {
            path,
            missing_is_noop: false,
        }));
    }

    let path = if configmap {
        component_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(super::source::configmap::DEFAULT_MOUNT_DIR))
            .join("shared.json")
    } else {
        PathBuf::from("/etc/edgecommons/shared.json")
    };
    validate_configmap_base_key(&path, configmap)?;
    Ok(Some(BasePathCandidate {
        path,
        missing_is_noop: true,
    }))
}

fn validate_configmap_base_key(path: &Path, configmap: bool) -> Result<()> {
    if configmap {
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if super::source::configmap::is_projection_artifact(name) {
                return Err(EdgeCommonsError::Config(format!(
                    "ConfigMap shared config key must not be a projection artifact: {name}"
                )));
            }
        }
    }
    Ok(())
}

enum BaseWatchTarget {
    File(PathBuf),
    ConfigMap(PathBuf),
    #[cfg(feature = "greengrass")]
    Shadow {
        thing_name: String,
    },
}

struct BaseWatch {
    rx: UnboundedReceiver<()>,
    guard: BaseWatchGuard,
}

enum BaseWatchGuard {
    File { _watcher: RecommendedWatcher },
    Task(tokio::task::JoinHandle<()>),
}

impl Drop for BaseWatchGuard {
    fn drop(&mut self) {
        match self {
            Self::File { .. } => {}
            Self::Task(handle) => handle.abort(),
        }
    }
}

fn watch_base(target: BaseWatchTarget) -> Option<BaseWatch> {
    match target {
        BaseWatchTarget::File(path) => watch_file_path(path),
        BaseWatchTarget::ConfigMap(path) => watch_configmap_path(path),
        #[cfg(feature = "greengrass")]
        BaseWatchTarget::Shadow { thing_name } => watch_shared_shadow(thing_name),
    }
}

fn watch_file_path(path: PathBuf) -> Option<BaseWatch> {
    let (tx, rx) = mpsc::unbounded_channel();
    let target = path.clone();
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let cb_target = target.clone();
    let mut watcher = recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            return;
        }
        if event
            .paths
            .iter()
            .any(|p| p.file_name() == cb_target.file_name())
        {
            let _ = tx.send(());
        }
    })
    .ok()?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some(BaseWatch {
        rx,
        guard: BaseWatchGuard::File { _watcher: watcher },
    })
}

fn watch_configmap_path(path: PathBuf) -> Option<BaseWatch> {
    let dir = path.parent()?.to_path_buf();
    let key = path.file_name()?.to_str()?.to_string();
    let source = super::source::configmap::ConfigMapConfigSource::new(Some(dir), Some(key)).ok()?;
    let mut updates = source.watch()?;
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let _source = source;
        while updates.recv().await.is_some() {
            if tx.send(()).is_err() {
                break;
            }
        }
    });
    Some(BaseWatch {
        rx,
        guard: BaseWatchGuard::Task(handle),
    })
}

#[cfg(feature = "greengrass")]
fn watch_shared_shadow(thing_name: String) -> Option<BaseWatch> {
    use crate::messaging::{Destination, Qos};

    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let rt = crate::ipc::global();
        if let Err(e) = rt.connect().await {
            tracing::warn!(error = %e, "shared SHADOW watch: connect failed");
            return;
        }
        let filter = format!("$aws/things/{thing_name}/shadow/name/{SHARED_SHADOW_NAME}/+/+");
        let (event_tx, mut event_rx) = mpsc::channel::<(String, Vec<u8>)>(16);
        if let Err(e) = rt
            .subscribe(&filter, Destination::Local, Qos::AtLeastOnce, event_tx)
            .await
        {
            tracing::warn!(error = %e, "shared SHADOW watch: subscribe failed");
            return;
        }
        while let Some((topic, _payload)) = event_rx.recv().await {
            let mut suffix = topic.rsplit('/');
            let result = suffix.next().unwrap_or("");
            let action = suffix.next().unwrap_or("");
            if matches!(
                (action, result),
                ("update", "delta") | ("update", "accepted")
            ) {
                if tx.send(()).is_err() {
                    break;
                }
            }
        }
    });
    Some(BaseWatch {
        rx,
        guard: BaseWatchGuard::Task(handle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vectors(name: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../split-config-test-vectors")
            .join(name);
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn case<'a>(doc: &'a Value, name: &str) -> &'a Value {
        doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
    }

    #[tokio::test]
    async fn consumes_merge_vectors_for_effective_merge_cases() {
        let doc = vectors("merge.json");
        for c in doc["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            if matches!(name, "base-extends-rejected" | "control-fields-stripped") {
                continue;
            }
            let input = &c["input"];
            let base = input.get("base").cloned();
            let component = input["component"].clone();
            let no_shared = input
                .get("options")
                .and_then(|o| o.get("noSharedConfig"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let effective = merge_bundle(LayerBundle { base, component }, no_shared).unwrap();
            assert_eq!(effective, c["expected"]["effective"], "{name}");
        }
    }

    #[test]
    fn pure_deep_merge_strips_control_fields() {
        let doc = vectors("merge.json");
        let c = case(&doc, "control-fields-stripped");
        let input = &c["input"];
        assert_eq!(
            deep_merge(&[input["base"].clone(), input["component"].clone()]),
            c["expected"]["effective"]
        );
    }

    #[test]
    fn coordinator_rejects_base_extends_in_v1() {
        let doc = vectors("merge.json");
        let c = case(&doc, "base-extends-rejected");
        let result = merge_bundle(
            LayerBundle {
                base: Some(c["input"]["base"].clone()),
                component: c["input"]["component"].clone(),
            },
            false,
        );
        assert!(result.unwrap_err().to_string().contains("N-layer"));
    }

    #[tokio::test]
    async fn consumes_config_component_bundle_vectors() {
        let doc = vectors("config-component-bundles.json");
        for c in doc["cases"].as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let payload = c["input"]
                .get("body")
                .or_else(|| c["input"].get("push"))
                .unwrap()
                .clone();
            let result = effective_from_source_payload(
                &ConfigSourceSpec::ConfigComponent,
                payload,
                false,
                "gw-01",
            )
            .await;
            if c["expected"].get("error").is_some() {
                assert!(result.is_err(), "{name}");
            } else if let Some(expected) = c["expected"].get("effective") {
                assert_eq!(result.unwrap(), *expected, "{name}");
            } else {
                assert!(
                    parse_config_component_payload(c["input"]["body"].clone()).is_ok(),
                    "{name}"
                );
            }
        }
    }

    #[tokio::test]
    async fn config_component_legacy_push_preserves_previous_base_only_for_pushes() {
        let initial = effective_candidate_from_source_payload(
            &ConfigSourceSpec::ConfigComponent,
            json!({
                "base": { "logging": { "level": "INFO" }, "tags": { "site": "dallas" } },
                "component": { "component": { "global": { "v": 1 } } }
            }),
            false,
            "gw-01",
            None,
        )
        .await
        .unwrap();
        assert_eq!(initial.effective["logging"]["level"], "INFO");

        let pushed = effective_candidate_from_source_payload(
            &ConfigSourceSpec::ConfigComponent,
            json!({
                "component": { "global": { "v": 2 } },
                "tags": { "component": "split" }
            }),
            false,
            "gw-01",
            initial.base.clone(),
        )
        .await
        .unwrap();
        assert_eq!(pushed.effective["logging"]["level"], "INFO");
        assert_eq!(pushed.effective["tags"]["site"], "dallas");
        assert_eq!(pushed.effective["tags"]["component"], "split");
        assert_eq!(pushed.effective["component"]["global"]["v"], 2);

        let refetch = effective_from_source_payload(
            &ConfigSourceSpec::ConfigComponent,
            json!({
                "component": { "global": { "v": 2 } },
                "tags": { "component": "split" }
            }),
            false,
            "gw-01",
        )
        .await
        .unwrap();
        assert!(refetch.get("logging").is_none());

        let explicit_null_base = effective_candidate_from_source_payload(
            &ConfigSourceSpec::ConfigComponent,
            json!({
                "base": null,
                "component": { "component": { "global": { "v": 3 } } }
            }),
            false,
            "gw-01",
            pushed.base,
        )
        .await
        .unwrap();
        assert!(explicit_null_base.effective.get("logging").is_none());
        assert_eq!(explicit_null_base.effective["component"]["global"]["v"], 3);
    }

    #[tokio::test]
    async fn file_and_configmap_resolution_follow_vectors() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("shared.json");
        std::fs::write(&base, r#"{"logging":{"level":"INFO"}}"#).unwrap();
        let component_path = root.path().join("config.json");
        let component = json!({ "extends": "shared.json", "component": {} });

        let resolved = resolve_base(
            &ConfigSourceSpec::File {
                path: component_path.clone(),
            },
            &component,
            false,
            "gw-01",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resolved["logging"]["level"], "INFO");

        let resolved = resolve_base(
            &ConfigSourceSpec::ConfigMap {
                mount_dir: Some(root.path().to_path_buf()),
                key: Some("config.json".into()),
            },
            &component,
            false,
            "gw-01",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(resolved["logging"]["level"], "INFO");

        let doc = vectors("resolution.json");
        assert_eq!(
            case(&doc, "file-extends-relative")["expected"]["basePath"],
            "/etc/edgecommons/shared.json"
        );
        assert_eq!(
            case(&doc, "configmap-mounted-shared-default")["expected"]["basePath"],
            "/var/run/edgecommons-config/shared.json"
        );
    }

    #[tokio::test]
    async fn env_resolution_supports_inline_json_and_at_path() {
        let var = "EDGECOMMONS_SHARED_CONFIG";
        unsafe { std::env::set_var(var, r#"{"logging":{"level":"INFO"}}"#) };
        let inline = resolve_env_base().await.unwrap().unwrap();
        assert_eq!(inline["logging"]["level"], "INFO");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.json");
        std::fs::write(&path, r#"{"logging":{"level":"WARN"}}"#).unwrap();
        unsafe { std::env::set_var(var, format!("@{}", path.display())) };
        let from_path = resolve_env_base().await.unwrap().unwrap();
        assert_eq!(from_path["logging"]["level"], "WARN");
        unsafe { std::env::remove_var(var) };
    }

    #[test]
    fn invalid_shared_config_control_is_rejected() {
        let result = merge_bundle(
            LayerBundle {
                base: None,
                component: json!({ "sharedConfig": "nope", "component": {} }),
            },
            false,
        );
        assert!(result.unwrap_err().to_string().contains("sharedConfig"));
    }

    #[tokio::test]
    async fn malformed_extends_is_rejected_for_every_source_family() {
        async fn assert_rejects(spec: ConfigSourceSpec) {
            let result = effective_from_source_payload(
                &spec,
                json!({ "extends": false, "component": {} }),
                true,
                "gw-01",
            )
            .await;
            assert!(result.unwrap_err().to_string().contains("extends"));
        }

        assert_rejects(ConfigSourceSpec::File {
            path: PathBuf::from("config.json"),
        })
        .await;
        assert_rejects(ConfigSourceSpec::ConfigMap {
            mount_dir: None,
            key: None,
        })
        .await;
        assert_rejects(ConfigSourceSpec::Env {
            var: "CONFIG".to_string(),
        })
        .await;
        assert_rejects(ConfigSourceSpec::ConfigComponent).await;
        #[cfg(feature = "greengrass")]
        {
            assert_rejects(ConfigSourceSpec::Greengrass {
                component: None,
                key: "ComponentConfig".to_string(),
            })
            .await;
            assert_rejects(ConfigSourceSpec::Shadow { name: None }).await;
        }

        let ok = effective_from_source_payload(
            &ConfigSourceSpec::Env {
                var: "CONFIG".to_string(),
            },
            json!({ "extends": "shared.json", "component": {} }),
            true,
            "gw-01",
        )
        .await
        .unwrap();
        assert_eq!(ok["component"], json!({}));
    }

    #[tokio::test]
    async fn inherited_streaming_definition_is_ordinary_effective_config() {
        let doc = vectors("merge.json");
        let c = case(&doc, "inherited-streaming-streams");
        let effective = merge_bundle(
            LayerBundle {
                base: Some(c["input"]["base"].clone()),
                component: c["input"]["component"].clone(),
            },
            false,
        )
        .unwrap();
        assert_eq!(
            effective["streaming"],
            c["expected"]["effective"]["streaming"]
        );
    }
}
