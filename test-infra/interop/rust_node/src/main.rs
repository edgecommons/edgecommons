//! Cross-language interop node (Rust) for edgecommons. See python_node.py for the
//! shared CLI contract:
//!   interop-rust-node responder <request_topic>
//!   interop-rust-node request   <request_topic> <token>
//!   interop-rust-node uns-pub   <identityJson> <class> [channel]
//!   interop-rust-node uns-sub   <topic>
//!   interop-rust-node uns-guard
//!   interop-rust-node status-responder    <component>
//!   interop-rust-node status-request      <component>
//!   interop-rust-node state-instances-pub <component>
//!   interop-rust-node state-instances-sub <component>
//!   interop-rust-node gg-config-request <topic> <component> <output-json>
//!   interop-rust-node gg-config-update <topic> <output-json>
//! Local-only MQTT transport against localhost:1883. Messages are built without a
//! config — the envelope legally omits `identity` unless one is stamped explicitly
//! (the UNS roles); `tags.thing` no longer exists (UNS hard cut).

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use edgecommons::prelude::{
    outcome_handler, CommandError, CommandOutcome, EdgeCommonsBuilder, InstanceConnectivity,
    InstanceConnectivityProvider, LogLevel, LogRecord,
};
use serde_json::json;
#[cfg(feature = "greengrass")]
use serde_json::{Map, Value};

use edgecommons::error::EdgeCommonsError;
use edgecommons::messaging::config::MessagingConfig;
#[cfg(feature = "greengrass")]
use edgecommons::messaging::message::Message;
use edgecommons::messaging::message::{
    binary_value, HierEntry, MessageBodyCase, MessageBuilder, MessageIdentity,
};
use edgecommons::messaging::message_handler;
#[cfg(feature = "greengrass")]
use edgecommons::messaging::provider::ipc::IpcProvider;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::service::{DefaultMessagingService, MessagingService};
use edgecommons::uns::{Uns, UnsClass};

const LANG: &str = "rust";

/// The fixed thing/device name every interop node runs under. The `status` / `state` roles are
/// addressed by component token alone, so the device must be identical in all four nodes.
const INTEROP_DEVICE: &str = "interop-device";

static ACCEPTANCE_MARKER_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The canonical per-instance connectivity sample (`test_interop.EXPECTED_INSTANCES`) that every
/// language's provider reports verbatim, so a `status` pull and a `state` push can be compared
/// across any producer/consumer pair:
/// - `cam-01` — every optional member present, and an `attributes` bag holding an array, a string
///   and a number (the OPEN bag must survive a four-language JSON round-trip).
/// - `cam-02` — `connected=false` with the richer `BACKOFF` state a boolean cannot express.
/// - `cam-03` — the minimal element: no state, no detail, no attributes. Optional members must be
///   OMITTED, never emitted as null/empty.
fn interop_instance_connectivity() -> Vec<InstanceConnectivity> {
    let mut attributes = serde_json::Map::new();
    attributes.insert("capabilities".to_string(), json!(["ptz", "snapshot"]));
    attributes.insert("vendor".to_string(), json!("acme"));
    attributes.insert("retries".to_string(), json!(0));
    vec![
        InstanceConnectivity::new("cam-01", true, Some("rtsp://cam-01/stream".to_string()))
            .with_state("ONLINE")
            .with_attributes(attributes),
        InstanceConnectivity::new("cam-02", false, Some("connect timed out".to_string()))
            .with_state("BACKOFF"),
        InstanceConnectivity::of("cam-03", true),
    ]
}

/// The component-scope identity a requester/subscriber derives from the `<component>` argument
/// alone (D-U28: no instance token) — the fixed interop device and that component token, the same
/// identity the responder/publisher resolves from its own runtime config. The library-owned `state`
/// keepalive and `status` command inbox are component-scoped, so this mints
/// `ecv1/interop-device/{component}/{class}` with no instance segment.
fn interop_identity(component_token: &str) -> MessageIdentity {
    MessageIdentity::new(
        vec![HierEntry {
            level: "device".to_string(),
            value: INTEROP_DEVICE.to_string(),
        }],
        component_token,
        None,
    )
    .expect("valid interop identity")
}

/// The real UNS builder over that identity (`includeRoot=false`), used to mint the component's
/// `cmd/status` request topic and its reserved `state` topic.
fn interop_uns(component_token: &str) -> Uns {
    Uns::new(interop_identity(component_token), false)
}

#[cfg(feature = "greengrass")]
fn deep_merge_value(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Object(mut left), Value::Object(right)) => {
            for (key, right_value) in right {
                let merged = match left.remove(&key) {
                    Some(left_value) => deep_merge_value(left_value, right_value),
                    None => right_value,
                };
                left.insert(key, merged);
            }
            Value::Object(left)
        }
        (_, right) => right,
    }
}

#[cfg(feature = "greengrass")]
fn validate_lineage_bundle(
    token: &str,
    body: &Value,
    expected_catalog_version: Option<&str>,
) -> (bool, Value) {
    let mut errors = Vec::new();
    if body.get("base").is_some() {
        errors.push("old top-level base layer is present".to_string());
    }
    if body.get("lineageVersion").and_then(Value::as_i64) != Some(1) {
        errors.push("lineageVersion must be 1".to_string());
    }
    let catalog_version = body.get("catalogVersion").and_then(Value::as_str);
    if let Some(expected) = expected_catalog_version {
        if catalog_version != Some(expected) {
            errors.push(format!(
                "catalogVersion must be {expected}, got {}",
                catalog_version.unwrap_or("<missing>")
            ));
        }
    }
    if body.get("component").and_then(Value::as_str) != Some(token) {
        errors.push(format!(
            "component must be {token}, got {}",
            body.get("component")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        ));
    }

    let mut effective = Value::Object(Map::new());
    let mut layer_ids = Vec::new();
    match body.get("layers").and_then(Value::as_array) {
        Some(layers) if !layers.is_empty() => {
            for layer in layers {
                let id = layer
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("<missing>")
                    .to_string();
                layer_ids.push(Value::String(id));
                match layer.get("config") {
                    Some(Value::Object(_)) => {
                        effective = deep_merge_value(effective, layer["config"].clone());
                    }
                    Some(_) => errors.push("layer config must be an object".to_string()),
                    None => errors.push("layer config is missing".to_string()),
                }
            }
        }
        Some(_) => errors.push("layers must not be empty".to_string()),
        None => errors.push("layers must be an array".to_string()),
    }

    let embedded_token = effective
        .get("component")
        .and_then(|component| component.get("token"))
        .and_then(Value::as_str);
    let publish_interval = effective
        .get("component")
        .and_then(|component| component.get("global"))
        .and_then(|global| global.get("publish_interval"))
        .and_then(Value::as_i64);
    let identity = effective.get("identity").cloned().unwrap_or(Value::Null);

    (
        errors.is_empty(),
        json!({
            "ok": errors.is_empty(),
            "errors": errors,
            "catalogVersion": catalog_version,
            "layerIds": layer_ids,
            "embeddedComponentToken": embedded_token,
            "publishInterval": publish_interval,
            "identity": identity,
            "effective": effective,
        }),
    )
}

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex length must be even".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn provider(suffix: &str) -> Arc<DefaultMessagingService> {
    let host =
        std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let pid = std::process::id();
    let cfg = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "interop-{LANG}-{suffix}-{pid}" }} }} }}"#
    );
    let mc: MessagingConfig = serde_json::from_str(&cfg).expect("valid config");
    let provider = MqttProvider::connect(&mc)
        .await
        .expect("connect to local broker");
    Arc::new(DefaultMessagingService::new(Arc::new(provider)))
}

fn log_component_token() -> String {
    format!("interop-log-{LANG}")
}

fn write_command_runtime_config(component_token: &str) -> std::path::PathBuf {
    let host =
        std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT")
        .unwrap_or_else(|_| "1883".to_string())
        .parse::<u16>()
        .expect("valid MQTT port");
    let path = std::env::temp_dir().join(format!(
        "edgecommons-deferred-{LANG}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let cfg = json!({
        "component": { "token": component_token },
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": host,
                "port": port,
                "clientId": format!("interop-{LANG}-deferred-runtime-{}", std::process::id())
            },
            "requestTimeoutSeconds": 4
        },
        "heartbeat": { "enabled": false },
        "health": { "enabled": false }
    });
    std::fs::write(&path, serde_json::to_vec(&cfg).expect("serialize config"))
        .expect("write deferred runtime config");
    path
}

/// Runtime config for `state-instances-pub`: the same local-MQTT bring-up as the command roles,
/// with the HEARTBEAT ENABLED so the component publishes its reserved `state` keepalive (the PUSH
/// surface). A short interval keeps the interop run brisk; the measures ride the metric subsystem
/// and are irrelevant here.
fn write_state_runtime_config(component_token: &str) -> std::path::PathBuf {
    let host =
        std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT")
        .unwrap_or_else(|_| "1883".to_string())
        .parse::<u16>()
        .expect("valid MQTT port");
    let path = std::env::temp_dir().join(format!(
        "edgecommons-state-{LANG}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let cfg = json!({
        "component": { "token": component_token },
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": host,
                "port": port,
                "clientId": format!("interop-{LANG}-state-runtime-{}", std::process::id())
            },
            "requestTimeoutSeconds": 4
        },
        "heartbeat": { "enabled": true, "intervalSecs": 2, "destination": "local" },
        "health": { "enabled": false }
    });
    std::fs::write(&path, serde_json::to_vec(&cfg).expect("serialize config"))
        .expect("write state runtime config");
    path
}

fn write_durable_acceptance_marker() -> std::io::Result<std::path::PathBuf> {
    let sequence = ACCEPTANCE_MARKER_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let marker = std::env::temp_dir().join(format!(
        "edgecommons-p1-accept-{LANG}-{}-{sequence}.marker",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)?;
    if let Err(error) =
        std::io::Write::write_all(&mut file, b"accepted\n").and_then(|()| file.sync_all())
    {
        let _ = std::fs::remove_file(&marker);
        return Err(error);
    }
    Ok(marker)
}

fn remove_durable_acceptance_marker(marker: &std::path::Path) {
    let _ = std::fs::remove_file(marker);
}

fn write_log_runtime_config() -> std::path::PathBuf {
    let host =
        std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT")
        .unwrap_or_else(|_| "1883".to_string())
        .parse::<u16>()
        .expect("valid MQTT port");
    let path = std::env::temp_dir().join(format!(
        "edgecommons-log-{LANG}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cfg = json!({
        "component": { "token": log_component_token() },
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": host,
                "port": port,
                "clientId": format!("interop-{LANG}-log-runtime-{}", std::process::id())
            },
            "requestTimeoutSeconds": 2
        },
        "heartbeat": { "enabled": false },
        "health": { "enabled": false },
        "logging": {
            "level": "WARN",
            "publish": {
                "enabled": true,
                "destination": "local",
                "minLevel": "TRACE",
                "captureNative": false,
                "captureConsole": false,
                "redaction": { "enabled": false }
            }
        }
    });
    std::fs::write(&path, serde_json::to_vec(&cfg).expect("serialize config"))
        .expect("write log runtime config");
    path
}

fn log_runtime_args(path: &std::path::Path) -> Vec<String> {
    let path = path.to_string_lossy().to_string();
    vec![
        "interop-rust-node".to_string(),
        "--platform".to_string(),
        "HOST".to_string(),
        "--transport".to_string(),
        "MQTT".to_string(),
        path.clone(),
        "-c".to_string(),
        "FILE".to_string(),
        path,
        "-t".to_string(),
        INTEROP_DEVICE.to_string(),
    ]
}

#[cfg(feature = "greengrass")]
fn gg_topic(run_id: &str, publisher: &str, subscriber: &str) -> String {
    format!("edgecommons/interop/binary/{run_id}/{publisher}/{subscriber}")
}

#[cfg(feature = "greengrass")]
fn gg_typed_topic(run_id: &str, publisher: &str, subscriber: &str) -> String {
    format!("edgecommons/interop/typed/{run_id}/{publisher}/{subscriber}")
}

fn typed_body(bytes: &[u8]) -> serde_json::Value {
    json!({
        "signal": { "id": "camera-1/roi-17/thumbnail", "name": "Thumbnail" },
        "samples": [{
            "value": binary_value(bytes).expect("binary sample marker"),
            "quality": "GOOD",
            "sourceTsMs": 1_783_360_799_900_u64,
            "serverTsMs": 1_783_360_800_000_u64
        }]
    })
}

#[cfg(feature = "greengrass")]
fn publisher_from_gg_topic(topic: &str) -> Option<String> {
    topic.split('/').rev().nth(1).map(ToString::to_string)
}

#[cfg(feature = "greengrass")]
fn typed_result(m: &Message, expected_bytes: &[u8], publisher: &str) -> Result<Value, String> {
    let sample = m
        .body
        .get("samples")
        .and_then(Value::as_array)
        .and_then(|samples| samples.first())
        .ok_or_else(|| "missing samples[0]".to_string())?;
    let marker = sample
        .get("value")
        .and_then(|value| value.get("_edgecommonsBinary"))
        .ok_or_else(|| "missing binary sample marker".to_string())?;
    let data = marker
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing binary sample data".to_string())?;
    let sample_bytes = BASE64_STANDARD.decode(data).map_err(|e| e.to_string())?;
    let source_ts_ms = sample.get("sourceTsMs").and_then(Value::as_u64);
    let server_ts_ms = sample.get("serverTsMs").and_then(Value::as_u64);
    let tag_from = m
        .tags
        .as_ref()
        .and_then(|tags| tags.extra.get("from"))
        .and_then(Value::as_str);
    let body_case = m.body_case().as_str();
    let ok = body_case == MessageBodyCase::SouthboundSignalUpdate.as_str()
        && sample_bytes == expected_bytes
        && source_ts_ms == Some(1_783_360_799_900)
        && server_ts_ms == Some(1_783_360_800_000)
        && tag_from == Some(publisher);
    Ok(json!({
        "body_case": body_case,
        "hex": encode_hex(&sample_bytes),
        "source_ts_ms": source_ts_ms,
        "server_ts_ms": server_ts_ms,
        "tag_from": tag_from,
        "ok": ok,
    }))
}

#[cfg(feature = "greengrass")]
fn gg_ready_path(run_id: &str, lang: &str) -> String {
    format!("/tmp/edgecommons_gg_ipc_binary_ready_{lang}_{run_id}")
}

#[cfg(feature = "greengrass")]
async fn ipc_provider() -> Arc<DefaultMessagingService> {
    let provider = IpcProvider::connect()
        .await
        .expect("connect to Greengrass IPC");
    Arc::new(DefaultMessagingService::new(Arc::new(provider)))
}

#[cfg(feature = "greengrass")]
async fn wait_for_gg_ready(run_id: &str, expected_langs: &[String]) -> Vec<String> {
    let ready_wait_secs: f64 = std::env::var("EDGECOMMONS_GG_READY_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180.0);
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(ready_wait_secs);
    while std::time::Instant::now() < deadline {
        let missing: Vec<String> = expected_langs
            .iter()
            .filter(|lang| !std::path::Path::new(&gg_ready_path(run_id, lang)).exists())
            .cloned()
            .collect();
        if missing.is_empty() {
            return missing;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    expected_langs
        .iter()
        .filter(|lang| !std::path::Path::new(&gg_ready_path(run_id, lang)).exists())
        .cloned()
        .collect()
}

#[cfg(feature = "greengrass")]
fn gg_log_ready_path(run_id: &str, lang: &str) -> String {
    format!("/tmp/edgecommons_gg_ipc_log_ready_{lang}_{run_id}")
}

#[cfg(feature = "greengrass")]
async fn wait_for_gg_log_ready(run_id: &str, expected_langs: &[String]) -> Vec<String> {
    let ready_wait_secs: f64 = std::env::var("EDGECOMMONS_GG_READY_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180.0);
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(ready_wait_secs);
    while std::time::Instant::now() < deadline {
        let missing: Vec<String> = expected_langs
            .iter()
            .filter(|lang| !std::path::Path::new(&gg_log_ready_path(run_id, lang)).exists())
            .cloned()
            .collect();
        if missing.is_empty() {
            return missing;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    expected_langs
        .iter()
        .filter(|lang| !std::path::Path::new(&gg_log_ready_path(run_id, lang)).exists())
        .cloned()
        .collect()
}

#[cfg(feature = "greengrass")]
fn gg_log_runtime_args(path: &std::path::Path) -> Vec<String> {
    let path = path.to_string_lossy().to_string();
    vec![
        "interop-rust-node".to_string(),
        "--platform".to_string(),
        "GREENGRASS".to_string(),
        "--transport".to_string(),
        "IPC".to_string(),
        "-c".to_string(),
        "FILE".to_string(),
        path,
        "-t".to_string(),
        "interop-device".to_string(),
    ]
}

#[cfg(feature = "greengrass")]
async fn run_gg_log_matrix(args: &[String]) -> ! {
    use std::collections::{BTreeMap, BTreeSet};

    let run_id = args[2].clone();
    let expected_langs: Vec<String> = args[3]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let expected: BTreeSet<String> = expected_langs.iter().cloned().collect();
    let ready_langs: Vec<String> = std::env::var("EDGECOMMONS_GG_READY_LANGS")
        .unwrap_or_else(|_| args[3].clone())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let ready_lang =
        std::env::var("EDGECOMMONS_GG_READY_LANG").unwrap_or_else(|_| LANG.to_string());
    let subscribe_delay_secs: f64 = std::env::var("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8.0);
    let wait_secs: f64 = std::env::var("EDGECOMMONS_GG_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(35.0);

    let svc = ipc_provider().await;
    let received = Arc::new(std::sync::Mutex::new(
        BTreeMap::<String, serde_json::Value>::new(),
    ));
    let errors = Arc::new(std::sync::Mutex::new(BTreeMap::<String, String>::new()));
    let rh = received.clone();
    let eh = errors.clone();
    let run_id_for_handler = run_id.clone();
    let expected_for_handler = expected.clone();
    svc.subscribe(
        "ecv1/interop-device/+/log/warn",
        message_handler(move |topic, m| {
            let rh = rh.clone();
            let eh = eh.clone();
            let run_id = run_id_for_handler.clone();
            let expected = expected_for_handler.clone();
            async move {
                let identity = m.identity.as_ref();
                let component = identity.map(|id| id.component()).unwrap_or("");
                let publisher = component.strip_prefix("interop-log-").unwrap_or(component);
                let fields = &m.body["fields"];
                let expected_logger = format!("interop.{publisher}");
                let expected_message = format!("gg-log-interop-{run_id}-{publisher}");
                let ok = expected.contains(publisher)
                    && identity.is_some_and(|id| {
                        // D-U28: the LogService publishes component-scope, so the record's
                        // identity carries the device and component but NO instance token
                        // (omitted on the wire). Asserting `instance() == None` proves the
                        // omit-when-absent contract survives the real Greengrass IPC hop.
                        id.device() == "interop-device" && id.instance().is_none()
                    })
                    && m.body["schema"].as_str() == Some("edgecommons.log.v1")
                    && m.body["level"].as_str() == Some("WARN")
                    && m.body["logger"].as_str() == Some(expected_logger.as_str())
                    && m.body["message"].as_str() == Some(expected_message.as_str())
                    && fields["runId"].as_str() == Some(run_id.as_str())
                    && fields["publisher"].as_str() == Some(publisher);
                if !publisher.is_empty() {
                    rh.lock().unwrap().entry(publisher.to_string()).or_insert_with(
                        || json!({"ok": ok, "topic": topic, "identity": m.identity, "body": m.body}),
                    );
                } else {
                    eh.lock()
                        .unwrap()
                        .insert(format!("log:{topic}"), "missing publisher identity".to_string());
                }
            }
        }),
        64,
        1,
    )
    .await
    .expect("subscribe log");

    println!("READY");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::fs::write(gg_log_ready_path(&run_id, &ready_lang), "ready").expect("write ready file");

    let ready_missing = wait_for_gg_log_ready(&run_id, &ready_langs).await;
    tokio::time::sleep(Duration::from_secs_f64(subscribe_delay_secs)).await;
    let mut published = json!({});
    if ready_missing.is_empty() {
        let path = write_log_runtime_config();
        let gg = EdgeCommonsBuilder::new(format!(
            "com.mbreissi.edgecommons.interop.{LANG}.LogPublisher"
        ))
        .args(gg_log_runtime_args(&path))
        .build()
        .await
        .expect("build EdgeCommons Greengrass log publisher");
        gg.logs()
            .publish(
                LogRecord::builder(
                    LogLevel::Warn,
                    format!("interop.{LANG}"),
                    format!("gg-log-interop-{run_id}-{LANG}"),
                )
                .field("runId", json!(run_id))
                .field("publisher", json!(LANG))
                .build(),
            )
            .await
            .expect("publish Greengrass log record");
        let stats = gg.logs().stats();
        published = json!({
            "published": stats.published,
            "failed": stats.failed,
            "queued": stats.queued,
            "dropped": stats.dropped
        });
        drop(gg);
        let _ = std::fs::remove_file(path);
    }

    let deadline = std::time::Instant::now() + Duration::from_secs_f64(wait_secs);
    while std::time::Instant::now() < deadline {
        let keys: BTreeSet<String> = received.lock().unwrap().keys().cloned().collect();
        if expected.is_subset(&keys) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let received_snapshot = received.lock().unwrap().clone();
    let errors_snapshot = errors.lock().unwrap().clone();
    let missing: Vec<String> = expected
        .iter()
        .filter(|lang| !received_snapshot.contains_key(*lang))
        .cloned()
        .collect();
    let all_ok = expected.iter().all(|lang| {
        received_snapshot
            .get(lang)
            .and_then(|item| item.get("ok"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    });
    let ok = ready_missing.is_empty() && missing.is_empty() && errors_snapshot.is_empty() && all_ok;
    let result = json!({
        "ok": ok,
        "lang": LANG,
        "run_id": run_id,
        "ready_missing": ready_missing,
        "received": received_snapshot,
        "missing": missing,
        "errors": errors_snapshot,
        "published": published
    });
    let path = format!("/tmp/edgecommons_gg_ipc_log_{ready_lang}_{}.json", args[2]);
    std::fs::write(&path, serde_json::to_vec(&result).unwrap()).expect("write result");
    println!("{}", result);
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(feature = "greengrass")]
async fn run_gg_binary_matrix(args: &[String]) -> ! {
    use std::collections::{BTreeMap, BTreeSet};

    let run_id = args[2].clone();
    let expected_langs: Vec<String> = args[3]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let expected: BTreeSet<String> = expected_langs.iter().cloned().collect();
    let ready_langs: Vec<String> = std::env::var("EDGECOMMONS_GG_READY_LANGS")
        .unwrap_or_else(|_| args[3].clone())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let ready_lang =
        std::env::var("EDGECOMMONS_GG_READY_LANG").unwrap_or_else(|_| LANG.to_string());
    let expected_hex = args[4].to_lowercase();
    let expected_bytes = decode_hex(&expected_hex).expect("expected hex");
    let subscribe_delay_secs: f64 = std::env::var("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8.0);
    let wait_secs: f64 = std::env::var("EDGECOMMONS_GG_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(35.0);

    let svc = ipc_provider().await;
    let received = Arc::new(std::sync::Mutex::new(
        BTreeMap::<String, serde_json::Value>::new(),
    ));
    let received_typed = Arc::new(std::sync::Mutex::new(
        BTreeMap::<String, serde_json::Value>::new(),
    ));
    let errors = Arc::new(std::sync::Mutex::new(BTreeMap::<String, String>::new()));
    let rh = received.clone();
    let eh = errors.clone();
    let expected_for_handler = expected_bytes.clone();
    svc.subscribe(
        &gg_topic(&run_id, "+", LANG),
        message_handler(move |topic, m| {
            let rh = rh.clone();
            let eh = eh.clone();
            let expected_for_handler = expected_for_handler.clone();
            async move {
                let publisher =
                    publisher_from_gg_topic(&topic).unwrap_or_else(|| "unknown".to_string());
                match (m.is_binary_body(), m.binary_body()) {
                    (is_binary, Ok(Some(bytes))) => {
                        let ok = is_binary && bytes == expected_for_handler;
                        rh.lock().unwrap().entry(publisher).or_insert_with(
                            || json!({"is_binary": is_binary, "hex": encode_hex(&bytes), "ok": ok}),
                        );
                    }
                    (is_binary, Ok(None)) => {
                        rh.lock().unwrap().entry(publisher).or_insert_with(
                            || json!({"is_binary": is_binary, "hex": null, "ok": false}),
                        );
                    }
                    (is_binary, Err(e)) => {
                        eh.lock()
                            .unwrap()
                            .insert(format!("{publisher}:binary"), e.to_string());
                        rh.lock().unwrap().entry(publisher).or_insert_with(
                            || json!({"is_binary": is_binary, "hex": null, "ok": false}),
                        );
                    }
                }
            }
        }),
        64,
        1,
    )
    .await
    .expect("subscribe binary");
    let rth = received_typed.clone();
    let eth = errors.clone();
    let expected_for_typed_handler = expected_bytes.clone();
    svc.subscribe(
        &gg_typed_topic(&run_id, "+", LANG),
        message_handler(move |topic, m| {
            let rth = rth.clone();
            let eth = eth.clone();
            let expected_for_typed_handler = expected_for_typed_handler.clone();
            async move {
                let publisher =
                    publisher_from_gg_topic(&topic).unwrap_or_else(|| "unknown".to_string());
                match typed_result(&m, &expected_for_typed_handler, &publisher) {
                    Ok(item) => {
                        rth.lock().unwrap().entry(publisher).or_insert(item);
                    }
                    Err(e) => {
                        eth.lock().unwrap().insert(format!("{publisher}:typed"), e);
                        rth.lock().unwrap().entry(publisher).or_insert_with(
                            || json!({"body_case": null, "hex": null, "ok": false}),
                        );
                    }
                }
            }
        }),
        64,
        1,
    )
    .await
    .expect("subscribe typed");
    println!("READY");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::fs::write(
        gg_ready_path(&run_id, &ready_lang),
        format!("{:?}", std::time::SystemTime::now()),
    )
    .expect("write ready");
    let ready_missing = wait_for_gg_ready(&run_id, &ready_langs).await;
    tokio::time::sleep(Duration::from_secs_f64(subscribe_delay_secs)).await;

    if ready_missing.is_empty() {
        let msg = MessageBuilder::new("InteropBinary", "1.0")
            .binary_payload(&expected_bytes)
            .expect("binary payload")
            .tag("from", json!(LANG))
            .build();
        let typed_msg = MessageBuilder::new("SouthboundSignalUpdate", "1.0")
            .southbound_signal_update(typed_body(&expected_bytes))
            .tag("from", json!(LANG))
            .build();
        for target in &expected_langs {
            svc.publish(&gg_topic(&run_id, LANG, target), &msg)
                .await
                .expect("publish binary");
            svc.publish(&gg_typed_topic(&run_id, LANG, target), &typed_msg)
                .await
                .expect("publish typed");
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs_f64(wait_secs);
    while std::time::Instant::now() < deadline {
        let got: BTreeSet<String> = received.lock().unwrap().keys().cloned().collect();
        let got_typed: BTreeSet<String> = received_typed.lock().unwrap().keys().cloned().collect();
        if expected.is_subset(&got) && expected.is_subset(&got_typed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let received_snapshot = received.lock().unwrap().clone();
    let received_typed_snapshot = received_typed.lock().unwrap().clone();
    let errors_snapshot = errors.lock().unwrap().clone();
    let missing: Vec<String> = expected_langs
        .iter()
        .filter(|lang| !received_snapshot.contains_key(*lang))
        .cloned()
        .collect();
    let missing_typed: Vec<String> = expected_langs
        .iter()
        .filter(|lang| !received_typed_snapshot.contains_key(*lang))
        .cloned()
        .collect();
    let ok = ready_missing.is_empty()
        && missing.is_empty()
        && missing_typed.is_empty()
        && errors_snapshot.is_empty()
        && expected_langs.iter().all(|lang| {
            received_snapshot
                .get(lang)
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                && received_typed_snapshot
                    .get(lang)
                    .and_then(|v| v.get("ok"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
        });
    let result = json!({
        "ok": ok,
        "lang": LANG,
        "run_id": run_id,
        "expected_hex": expected_hex,
        "ready_missing": ready_missing,
        "received": received_snapshot,
        "received_typed": received_typed_snapshot,
        "missing": missing,
        "missing_typed": missing_typed,
        "errors": errors_snapshot,
    });
    let path = format!("/tmp/edgecommons_gg_ipc_binary_{LANG}_{}.json", args[2]);
    std::fs::write(&path, serde_json::to_string(&result).unwrap()).expect("write result");
    println!("{result}");
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(feature = "greengrass")]
fn gg_p1_ready_path(run_id: &str, actor: &str) -> String {
    format!("/tmp/edgecommons_gg_ipc_p1_ready_{actor}_{run_id}")
}

#[cfg(feature = "greengrass")]
async fn wait_for_gg_p1_ready(run_id: &str, expected_actors: &[String]) -> Vec<String> {
    let ready_wait_secs: f64 = std::env::var("EDGECOMMONS_GG_READY_WAIT_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(180.0);
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(ready_wait_secs);
    while std::time::Instant::now() < deadline {
        let missing: Vec<String> = expected_actors
            .iter()
            .filter(|actor| !std::path::Path::new(&gg_p1_ready_path(run_id, actor)).exists())
            .cloned()
            .collect();
        if missing.is_empty() {
            return missing;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    expected_actors
        .iter()
        .filter(|actor| !std::path::Path::new(&gg_p1_ready_path(run_id, actor)).exists())
        .cloned()
        .collect()
}

#[cfg(feature = "greengrass")]
fn gg_p1_target_actor(target_language: &str, sender_actor: &str) -> String {
    if target_language == "rust" && sender_actor == "rust" {
        "rustpeer".to_string()
    } else {
        target_language.to_string()
    }
}

#[cfg(feature = "greengrass")]
fn gg_p1_command_topic(actor: &str) -> String {
    format!("ecv1/interop-device/interop-p1-{actor}/cmd/deferred")
}

#[cfg(feature = "greengrass")]
fn gg_p1_confirmed_topic(run_id: &str, publisher: &str, target_actor: &str) -> String {
    format!("edgecommons/interop/p1/{run_id}/confirmed/{publisher}/{target_actor}")
}

#[cfg(feature = "greengrass")]
async fn send_gg_p1_deferred(
    svc: &Arc<DefaultMessagingService>,
    run_id: &str,
    sender_actor: &str,
    target_language: &str,
    target_actor: &str,
) -> Value {
    let token = format!("{run_id}:{sender_actor}->{target_language}");
    let reply_topic = format!(
        "edgecommons/interop/p1/{run_id}/reply/{sender_actor}/{target_actor}/{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    let replies = Arc::new(std::sync::Mutex::new(Vec::<Message>::new()));
    let received = replies.clone();
    if let Err(error) = svc
        .subscribe(
            &reply_topic,
            message_handler(move |_topic, reply| {
                let received = received.clone();
                async move {
                    received.lock().unwrap().push(reply);
                }
            }),
            2,
            1,
        )
        .await
    {
        return json!({"ok": false, "target_actor": target_actor, "error": error.to_string()});
    }
    let request = MessageBuilder::new("deferred", "1.0")
        .command(json!({"token": token, "from": LANG, "actor": sender_actor}))
        .reply_to(&reply_topic)
        .build();
    let correlation = request.header.correlation_id.clone();
    if let Err(error) = svc
        .publish(&gg_p1_command_topic(target_actor), &request)
        .await
    {
        return json!({"ok": false, "target_actor": target_actor, "error": error.to_string()});
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline && replies.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if replies.lock().unwrap().is_empty() {
        return json!({"ok": false, "target_actor": target_actor, "error": "timeout"});
    }
    tokio::time::sleep(Duration::from_millis(750)).await;
    let replies = replies.lock().unwrap();
    let reply_count = replies.len();
    let reply = &replies[0];
    let correlation_match = reply.header.correlation_id == correlation;
    let result = reply.body.get("result");
    let ok = reply_count == 1
        && correlation_match
        && reply.body.get("ok").and_then(Value::as_bool) == Some(true)
        && result
            .and_then(|value| value.get("token"))
            .and_then(Value::as_str)
            == Some(token.as_str())
        && result
            .and_then(|value| value.get("durablyAccepted"))
            .and_then(Value::as_bool)
            == Some(true)
        && result
            .and_then(|value| value.get("responder"))
            .and_then(Value::as_str)
            == Some(target_language)
        && result
            .and_then(|value| value.get("responderActor"))
            .and_then(Value::as_str)
            == Some(target_actor);
    json!({
        "ok": ok,
        "target_actor": target_actor,
        "expected_token": token,
        "expected_responder": target_language,
        "expected_responder_actor": target_actor,
        "reply_count": reply_count,
        "correlation_match": correlation_match,
        "duplicate_window_ms": 750,
        "reply_body": reply.body
    })
}

#[cfg(feature = "greengrass")]
async fn run_gg_p1_matrix(args: &[String]) -> ! {
    use std::collections::{BTreeMap, BTreeSet};

    let run_id = args[2].clone();
    let languages: Vec<String> = args[3]
        .split(',')
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect();
    let expected_actors: Vec<String> = std::env::var("EDGECOMMONS_GG_READY_LANGS")
        .unwrap_or_else(|_| args[3].clone())
        .split(',')
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect();
    let actor = std::env::var("EDGECOMMONS_GG_READY_LANG").unwrap_or_else(|_| LANG.to_string());
    let canonical_actor = actor != "rustpeer";
    let subscribe_delay_secs: f64 = std::env::var("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2.0);
    let wait_secs: f64 = std::env::var("EDGECOMMONS_GG_WAIT_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(90.0);
    let expected_publishers: BTreeSet<String> = if actor == "rust" {
        languages
            .iter()
            .filter(|value| value.as_str() != "rust")
            .cloned()
            .collect()
    } else if canonical_actor {
        languages.iter().cloned().collect()
    } else {
        BTreeSet::from(["rust".to_string()])
    };

    let svc = ipc_provider().await;
    let received = Arc::new(std::sync::Mutex::new(BTreeMap::<String, Vec<Value>>::new()));
    let errors = Arc::new(std::sync::Mutex::new(BTreeMap::<String, String>::new()));
    let received_handler = received.clone();
    let errors_handler = errors.clone();
    let run_id_handler = run_id.clone();
    let actor_handler = actor.clone();
    if let Err(error) = svc
        .subscribe(
            &format!("edgecommons/interop/p1/{run_id}/confirmed/+/{actor}"),
            message_handler(move |topic, message| {
                let received = received_handler.clone();
                let errors = errors_handler.clone();
                let run_id = run_id_handler.clone();
                let actor = actor_handler.clone();
                async move {
                    let publisher =
                        publisher_from_gg_topic(&topic).unwrap_or_else(|| "unknown".to_string());
                    let body = message.body;
                    let valid = body.get("runId").and_then(Value::as_str) == Some(run_id.as_str())
                        && body.get("publisher").and_then(Value::as_str)
                            == Some(publisher.as_str())
                        && body.get("targetActor").and_then(Value::as_str) == Some(actor.as_str())
                        && body.get("strict").and_then(Value::as_bool) == Some(true);
                    if publisher == "unknown" {
                        errors.lock().unwrap().insert(
                            format!("confirmed:{topic}"),
                            "missing publisher topic segment".to_string(),
                        );
                    }
                    received
                        .lock()
                        .unwrap()
                        .entry(publisher)
                        .or_default()
                        .push(json!({
                            "ok": valid,
                            "topic": topic,
                            "body": body
                        }));
                }
            }),
            32,
            1,
        )
        .await
    {
        let result = json!({
            "schema": "edgecommons.gg-ipc-p1.v1",
            "ok": false,
            "run_id": run_id,
            "actor": actor,
            "language": LANG,
            "errors": {"subscribe": error.to_string()}
        });
        println!("{result}");
        std::process::exit(1);
    }

    let path = write_command_runtime_config(&format!("interop-p1-{actor}"));
    let runtime_args = gg_log_runtime_args(&path);
    let gg = EdgeCommonsBuilder::new(format!(
        "com.mbreissi.edgecommons.interop.{LANG}.P1Responder"
    ))
    .args(runtime_args)
    .build()
    .await
    .expect("build Greengrass P1 command responder");
    let inbox = gg.commands().expect("runtime command inbox");
    let responder_actor = actor.clone();
    inbox
        .register_outcome(
            "deferred",
            outcome_handler(move |request, deferred| {
                let responder_actor = responder_actor.clone();
                async move {
                    let token = match deferred.defer(&request, Duration::from_secs(4)) {
                        Ok(token) => token,
                        Err(error) => return CommandOutcome::ImmediateError(error),
                    };
                    let acceptance_marker = match write_durable_acceptance_marker() {
                        Ok(marker) => marker,
                        Err(_) => {
                            let _ = token.discard();
                            return CommandOutcome::ImmediateError(CommandError::new(
                                "ACCEPTANCE_FAILED",
                                "work was not accepted",
                            ));
                        }
                    };
                    let accepted_token = request.body.get("token").cloned().unwrap_or_default();
                    if let Err(error) = token.activate() {
                        remove_durable_acceptance_marker(&acceptance_marker);
                        let _ = token.discard();
                        return CommandOutcome::ImmediateError(CommandError::new(
                            "ACTIVATION_FAILED",
                            error.to_string(),
                        ));
                    }
                    let settlement = token.clone();
                    CommandOutcome::deferred_with_continuation(token, async move {
                        let settled = settlement
                            .settle_success(Some(json!({
                                "token": accepted_token,
                                "responder": LANG,
                                "responderActor": responder_actor,
                                "durablyAccepted": true
                            })))
                            .await
                            .map_err(|error| {
                                CommandError::new("SETTLEMENT_FAILED", error.to_string())
                            });
                        remove_durable_acceptance_marker(&acceptance_marker);
                        settled
                    })
                }
            }),
        )
        .expect("register Greengrass P1 command handler");
    println!("READY");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::fs::write(gg_p1_ready_path(&run_id, &actor), "ready").expect("write P1 ready");

    let ready_missing = wait_for_gg_p1_ready(&run_id, &expected_actors).await;
    let mut deferred_requests = BTreeMap::<String, Value>::new();
    let mut confirmed_publishes = BTreeMap::<String, Value>::new();
    if ready_missing.is_empty() && canonical_actor {
        tokio::time::sleep(Duration::from_secs_f64(subscribe_delay_secs)).await;
        for target_language in &languages {
            let target_actor = gg_p1_target_actor(target_language, &actor);
            let request =
                send_gg_p1_deferred(&svc, &run_id, &actor, target_language, &target_actor).await;
            deferred_requests.insert(target_language.clone(), request);
            let message = MessageBuilder::new("InteropConfirmed", "1.0")
                .payload(json!({
                    "runId": run_id,
                    "publisher": LANG,
                    "publisherActor": actor,
                    "targetLanguage": target_language,
                    "targetActor": target_actor,
                    "strict": true
                }))
                .build();
            let published = match svc
                .publish_confirmed(
                    &gg_p1_confirmed_topic(&run_id, LANG, &target_actor),
                    &message,
                    Duration::from_secs(5),
                )
                .await
            {
                Ok(()) => json!({
                    "ok": true, "target_actor": target_actor, "confirmed": true, "qos": 1
                }),
                Err(error) => json!({
                    "ok": false, "target_actor": target_actor, "error": error.to_string()
                }),
            };
            confirmed_publishes.insert(target_language.clone(), published);
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs_f64(wait_secs);
    while std::time::Instant::now() < deadline {
        let seen: BTreeSet<String> = received.lock().unwrap().keys().cloned().collect();
        if expected_publishers.is_subset(&seen) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(750)).await;
    let received_snapshot = received.lock().unwrap().clone();
    let errors_snapshot = errors.lock().unwrap().clone();
    let confirmed_missing: Vec<String> = expected_publishers
        .iter()
        .filter(|publisher| !received_snapshot.contains_key(*publisher))
        .cloned()
        .collect();
    let mut confirmed_received = BTreeMap::<String, Value>::new();
    let mut receive_ok = confirmed_missing.is_empty();
    for (publisher, items) in &received_snapshot {
        let item_ok = expected_publishers.contains(publisher)
            && items.len() == 1
            && items[0].get("ok").and_then(Value::as_bool) == Some(true);
        confirmed_received.insert(
            publisher.clone(),
            json!({"count": items.len(), "items": items, "ok": item_ok}),
        );
        receive_ok &= item_ok;
    }
    let requests_ok = !canonical_actor
        || (deferred_requests.len() == languages.len()
            && languages.iter().all(|language| {
                deferred_requests
                    .get(language)
                    .and_then(|value| value.get("ok"))
                    .and_then(Value::as_bool)
                    == Some(true)
            }));
    let publishes_ok = !canonical_actor
        || (confirmed_publishes.len() == languages.len()
            && languages.iter().all(|language| {
                confirmed_publishes
                    .get(language)
                    .and_then(|value| value.get("ok"))
                    .and_then(Value::as_bool)
                    == Some(true)
            }));
    let ok = ready_missing.is_empty()
        && errors_snapshot.is_empty()
        && requests_ok
        && publishes_ok
        && receive_ok;
    let result = json!({
        "schema": "edgecommons.gg-ipc-p1.v1",
        "ok": ok,
        "run_id": run_id,
        "actor": actor,
        "language": LANG,
        "canonical_actor": canonical_actor,
        "ready_missing": ready_missing,
        "deferred_requests": deferred_requests,
        "confirmed_publishes": confirmed_publishes,
        "confirmed_received": confirmed_received,
        "confirmed_missing": confirmed_missing,
        "errors": errors_snapshot
    });
    let result_path = format!("/tmp/edgecommons_gg_ipc_p1_{actor}_{run_id}.json");
    std::fs::write(&result_path, serde_json::to_vec(&result).unwrap()).expect("write P1 result");
    println!("{result}");
    drop(gg);
    let _ = std::fs::remove_file(path);
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_binary_matrix(_args: &[String]) -> ! {
    eprintln!("gg-binary-matrix requires the greengrass cargo feature");
    std::process::exit(2);
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_log_matrix(_args: &[String]) -> ! {
    eprintln!("gg-log-matrix requires the greengrass cargo feature");
    std::process::exit(2);
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_p1_matrix(_args: &[String]) -> ! {
    eprintln!("gg-p1-matrix requires the greengrass cargo feature");
    std::process::exit(2);
}

#[cfg(feature = "greengrass")]
async fn run_gg_config_request(args: &[String]) -> ! {
    if args.len() < 5 {
        eprintln!("gg-config-request requires <topic> <component> <output-json>");
        std::process::exit(2);
    }
    let topic = args[2].clone();
    let component = args[3].clone();
    let output = args[4].clone();
    let svc = ipc_provider().await;
    let req = MessageBuilder::new("GetConfiguration", "1.0")
        .payload(json!({ "component": component }))
        .build();
    let corr = req.header.correlation_id.clone();
    let fut = match svc.request(&topic, req).await {
        Ok(fut) => fut,
        Err(error) => {
            let result = json!({"ok": false, "error": format!("request failed: {error}")});
            let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
            println!("{result}");
            std::process::exit(1);
        }
    };
    let result = match tokio::time::timeout(Duration::from_secs(20), fut).await {
        Ok(Ok(reply)) => {
            let matched = reply.header.correlation_id == corr;
            let (lineage_ok, lineage_check) =
                validate_lineage_bundle(&component, &reply.body, None);
            json!({
                "ok": matched && lineage_ok,
                "correlation_match": matched,
                "lineage_ok": lineage_ok,
                "lineage_check": lineage_check,
                "reply_body": reply.body,
            })
        }
        Ok(Err(error)) => json!({"ok": false, "error": format!("reply failed: {error}")}),
        Err(_) => json!({"ok": false, "error": "timeout"}),
    };
    let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
    println!("{result}");
    std::process::exit(if result.get("ok").and_then(Value::as_bool) == Some(true) {
        0
    } else {
        1
    });
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_config_request(_args: &[String]) -> ! {
    eprintln!("gg-config-request requires the greengrass cargo feature");
    std::process::exit(2);
}

#[cfg(feature = "greengrass")]
async fn run_gg_config_update(args: &[String]) -> ! {
    if args.len() < 4 {
        eprintln!("gg-config-update requires <topic> <output-json>");
        std::process::exit(2);
    }
    let topic = args[2].clone();
    let output = args[3].clone();
    let svc = ipc_provider().await;
    let received = Arc::new(std::sync::Mutex::new(serde_json::Map::new()));

    for token in ["opcua-adapter", "modbus-adapter"] {
        let push_topic = format!("ecv1/lab-5950x/{token}/cmd/set-config");
        let received_for_handler = received.clone();
        let token_for_handler = token.to_string();
        svc.subscribe(
            &push_topic,
            message_handler(move |_topic, message| {
                let received_for_handler = received_for_handler.clone();
                let token_for_handler = token_for_handler.clone();
                async move {
                    if let Ok(mut guard) = received_for_handler.lock() {
                        guard.insert(token_for_handler, message.body);
                    }
                }
            }),
            16,
            1,
        )
        .await
        .expect("subscribe to pushed set-config");
    }

    let version = format!("smoke-{}", std::process::id());
    let catalog = json!({
        "schemaVersion": 1,
        "version": version,
        "provenance": {
            "source": "message",
            "uri": "greengrass-smoke"
        },
        "hierarchy": {
            "levels": ["enterprise", "site", "zone", "line", "device"]
        },
        "nodes": {
            "enterprise/acme": {
                "scope": { "enterprise": "acme" },
                "config": {
                    "hierarchy": {
                        "levels": ["enterprise", "site", "zone", "line", "device"]
                    },
                    "identity": { "enterprise": "acme" },
                    "logging": { "level": "INFO" }
                }
            },
            "site/dallas": {
                "parent": "enterprise/acme",
                "scope": { "enterprise": "acme", "site": "dallas" },
                "config": {
                    "identity": { "site": "dallas" },
                    "tags": { "site": "dallas" }
                }
            },
            "zone/smoke": {
                "parent": "site/dallas",
                "scope": { "enterprise": "acme", "site": "dallas", "zone": "smoke" },
                "config": {
                    "identity": { "zone": "smoke" }
                }
            },
            "line/line-7": {
                "parent": "zone/smoke",
                "scope": {
                    "enterprise": "acme",
                    "site": "dallas",
                    "zone": "smoke",
                    "line": "line-7"
                },
                "config": {
                    "identity": { "line": "line-7" }
                }
            }
        },
        "components": {
            "opcua-adapter": {
                "parent": "line/line-7",
                "config": {
                    "component": {
                        "token": "opcua-adapter",
                        "global": {
                            "endpoint": "opc.tcp://plc-1:4840"
                        },
                        "instances": []
                    }
                }
            },
            "modbus-adapter": {
                "parent": "line/line-7",
                "config": {
                    "component": {
                        "token": "modbus-adapter",
                        "global": {
                            "unitId": 1
                        },
                        "instances": []
                    }
                }
            }
        }
    });
    let req = MessageBuilder::new("UpdateCatalog", "1.0")
        .payload(json!({ "version": version, "catalog": catalog }))
        .build();
    let corr = req.header.correlation_id.clone();
    let fut = match svc.request(&topic, req).await {
        Ok(fut) => fut,
        Err(error) => {
            let result = json!({"ok": false, "error": format!("request failed: {error}")});
            let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
            println!("{result}");
            std::process::exit(1);
        }
    };
    let reply_result = tokio::time::timeout(Duration::from_secs(20), fut).await;
    let (ack_ok, correlation_match, ack_body) = match reply_result {
        Ok(Ok(reply)) => (
            reply.body.get("ok").and_then(Value::as_bool) == Some(true),
            reply.header.correlation_id == corr,
            reply.body,
        ),
        Ok(Err(error)) => (
            false,
            false,
            json!({"error": format!("reply failed: {error}")}),
        ),
        Err(_) => (false, false, json!({"error": "timeout"})),
    };

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let pushed_count = received.lock().map(|guard| guard.len()).unwrap_or_default();
        if pushed_count >= 2 || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let pushes = received
        .lock()
        .map(|guard| Value::Object(guard.clone()))
        .unwrap_or_else(|_| json!({"error": "received push map poisoned"}));
    let pushed_count = pushes.as_object().map(|map| map.len()).unwrap_or_default();
    let mut push_checks = Map::new();
    let mut pushes_valid = true;
    if let Some(push_map) = pushes.as_object() {
        for token in ["opcua-adapter", "modbus-adapter"] {
            match push_map.get(token) {
                Some(body) => {
                    let (ok, check) = validate_lineage_bundle(token, body, Some(&version));
                    pushes_valid &= ok;
                    push_checks.insert(token.to_string(), check);
                }
                None => {
                    pushes_valid = false;
                    push_checks.insert(
                        token.to_string(),
                        json!({"ok": false, "errors": ["missing set-config push"]}),
                    );
                }
            }
        }
    } else {
        pushes_valid = false;
    }
    let result = json!({
        "ok": ack_ok && correlation_match && pushed_count >= 2 && pushes_valid,
        "ack_ok": ack_ok,
        "correlation_match": correlation_match,
        "pushed_count": pushed_count,
        "ack_body": ack_body,
        "push_checks": Value::Object(push_checks),
        "pushes": pushes,
    });
    let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
    println!("{result}");
    std::process::exit(if result.get("ok").and_then(Value::as_bool) == Some(true) {
        0
    } else {
        1
    });
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_config_update(_args: &[String]) -> ! {
    eprintln!("gg-config-update requires the greengrass cargo feature");
    std::process::exit(2);
}

#[cfg(feature = "greengrass")]
async fn run_gg_config_update_file(args: &[String]) -> ! {
    if args.len() < 6 {
        eprintln!(
            "gg-config-update-file requires <topic> <catalog-json> <tokens-csv> <output-json>"
        );
        std::process::exit(2);
    }
    let topic = args[2].clone();
    let catalog_path = args[3].clone();
    let tokens: Vec<String> = args[4]
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let output = args[5].clone();
    let catalog_text = match std::fs::read_to_string(&catalog_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("failed to read catalog file {catalog_path}: {error}");
            std::process::exit(2);
        }
    };
    let catalog: Value = match serde_json::from_str(&catalog_text) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("catalog file {catalog_path} is not JSON: {error}");
            std::process::exit(2);
        }
    };
    let version = catalog
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("gg-config-update-file")
        .to_string();
    let svc = ipc_provider().await;
    let received = Arc::new(std::sync::Mutex::new(serde_json::Map::new()));

    for token in &tokens {
        let push_topic = format!("ecv1/lab-5950x/{token}/cmd/set-config");
        let received_for_handler = received.clone();
        let token_for_handler = token.clone();
        svc.subscribe(
            &push_topic,
            message_handler(move |_topic, message| {
                let received_for_handler = received_for_handler.clone();
                let token_for_handler = token_for_handler.clone();
                async move {
                    if let Ok(mut guard) = received_for_handler.lock() {
                        guard.insert(token_for_handler, message.body);
                    }
                }
            }),
            16,
            1,
        )
        .await
        .expect("subscribe to pushed set-config");
    }

    let req = MessageBuilder::new("UpdateCatalog", "1.0")
        .payload(json!({ "version": version, "catalog": catalog }))
        .build();
    let corr = req.header.correlation_id.clone();
    let fut = match svc.request(&topic, req).await {
        Ok(fut) => fut,
        Err(error) => {
            let result = json!({"ok": false, "error": format!("request failed: {error}")});
            let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
            println!("{result}");
            std::process::exit(1);
        }
    };
    let reply_result = tokio::time::timeout(Duration::from_secs(20), fut).await;
    let (ack_ok, correlation_match, ack_body) = match reply_result {
        Ok(Ok(reply)) => (
            reply.body.get("ok").and_then(Value::as_bool) == Some(true),
            reply.header.correlation_id == corr,
            reply.body,
        ),
        Ok(Err(error)) => (
            false,
            false,
            json!({"error": format!("reply failed: {error}")}),
        ),
        Err(_) => (false, false, json!({"error": "timeout"})),
    };

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let pushed_count = received.lock().map(|guard| guard.len()).unwrap_or_default();
        if pushed_count >= tokens.len() || std::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let pushes = received
        .lock()
        .map(|guard| Value::Object(guard.clone()))
        .unwrap_or_else(|_| json!({"error": "received push map poisoned"}));
    let pushed_count = pushes.as_object().map(|map| map.len()).unwrap_or_default();
    let mut push_checks = Map::new();
    let mut pushes_valid = true;
    if let Some(push_map) = pushes.as_object() {
        for token in &tokens {
            match push_map.get(token) {
                Some(body) => {
                    let (ok, check) = validate_lineage_bundle(token, body, Some(&version));
                    pushes_valid &= ok;
                    push_checks.insert(token.clone(), check);
                }
                None => {
                    pushes_valid = false;
                    push_checks.insert(
                        token.clone(),
                        json!({"ok": false, "errors": ["missing set-config push"]}),
                    );
                }
            }
        }
    } else {
        pushes_valid = false;
    }
    let result = json!({
        "ok": ack_ok && correlation_match && pushed_count >= tokens.len() && pushes_valid,
        "ack_ok": ack_ok,
        "correlation_match": correlation_match,
        "expected_pushes": tokens.len(),
        "pushed_count": pushed_count,
        "ack_body": ack_body,
        "push_checks": Value::Object(push_checks),
        "pushes": pushes,
    });
    let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
    println!("{result}");
    std::process::exit(if result.get("ok").and_then(Value::as_bool) == Some(true) {
        0
    } else {
        1
    });
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_config_update_file(_args: &[String]) -> ! {
    eprintln!("gg-config-update-file requires the greengrass cargo feature");
    std::process::exit(2);
}

#[cfg(feature = "greengrass")]
async fn run_gg_command_request(args: &[String]) -> ! {
    if args.len() < 5 {
        eprintln!("gg-command-request requires <topic> <name> <output-json>");
        std::process::exit(2);
    }
    let topic = args[2].clone();
    let name = args[3].clone();
    let output = args[4].clone();
    let svc = ipc_provider().await;
    let req = MessageBuilder::new(name, "1.0").payload(json!({})).build();
    let corr = req.header.correlation_id.clone();
    let fut = match svc.request(&topic, req).await {
        Ok(fut) => fut,
        Err(error) => {
            let result = json!({"ok": false, "error": format!("request failed: {error}")});
            let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
            println!("{result}");
            std::process::exit(1);
        }
    };
    let result = match tokio::time::timeout(Duration::from_secs(10), fut).await {
        Ok(Ok(reply)) => json!({
            "ok": true,
            "correlation_match": reply.header.correlation_id == corr,
            "reply_body": reply.body,
        }),
        Ok(Err(error)) => json!({"ok": false, "error": format!("reply failed: {error}")}),
        Err(_) => json!({"ok": false, "error": "timeout"}),
    };
    let ok = result.get("ok").and_then(Value::as_bool) == Some(true)
        && result.get("correlation_match").and_then(Value::as_bool) == Some(true);
    let _ = std::fs::write(&output, serde_json::to_string_pretty(&result).unwrap());
    println!("{result}");
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_command_request(_args: &[String]) -> ! {
    eprintln!("gg-command-request requires the greengrass cargo feature");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let role = args.get(1).map(String::as_str).unwrap_or("");

    match role {
        "responder" => {
            let topic = args[2].clone();
            let svc = provider("resp").await;
            let responder = svc.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, request| {
                    let responder = responder.clone();
                    async move {
                        let reply = MessageBuilder::new("InteropReply", "1.0")
                            .payload(json!({ "echo": request.body, "responder": LANG }))
                            .build();
                        if let Err(e) = responder.reply(&request, reply).await {
                            eprintln!("reply failed: {e}");
                        }
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
        "request" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("req").await;
            // Canonical cross-language payload permutations (echoed back by the responder;
            // test_interop asserts a deep round-trip). null is tested inside an array.
            let types = json!({
                "b": true, "bf": false,
                "i": 42, "ni": -7, "fl": 3.5,
                "slash": "a/b", "quote": "x\"y",
                "arr": [1, "two", false, null],
                "nullv": null,
                "nested": { "k": [1, { "d": 2 }] },
                "ea": [], "eo": {}
            });
            let req = MessageBuilder::new("InteropRequest", "1.0")
                .payload(json!({ "token": token, "from": LANG, "types": types }))
                .build();
            let corr = req.header.correlation_id.clone();
            let fut = svc.request(&topic, req).await.expect("request issued");
            match tokio::time::timeout(Duration::from_secs(8), fut).await {
                Ok(Ok(reply)) => {
                    let matched = reply.header.correlation_id == corr;
                    println!(
                        "{}",
                        json!({"ok": true, "correlation_match": matched, "reply_body": reply.body})
                    );
                    let echo_token = reply
                        .body
                        .get("echo")
                        .and_then(|e| e.get("token"))
                        .and_then(|t| t.as_str());
                    let ok = matched
                        && reply.body.get("responder").is_some()
                        && echo_token == Some(token.as_str());
                    std::process::exit(if ok { 0 } else { 1 });
                }
                _ => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "deferred-responder" => {
            let component_token = args[2].clone();
            let path = write_command_runtime_config(&component_token);
            let runtime_args = log_runtime_args(&path);
            let gg = EdgeCommonsBuilder::new(format!(
                "com.mbreissi.edgecommons.interop.{LANG}.DeferredResponder"
            ))
            .args(runtime_args)
            .build()
            .await
            .expect("build deferred command responder");
            let inbox = gg.commands().expect("runtime command inbox");
            inbox
                .register_outcome(
                    "deferred",
                    outcome_handler(|request, deferred| async move {
                        let token = match deferred.defer(&request, Duration::from_secs(4)) {
                            Ok(token) => token,
                            Err(error) => return CommandOutcome::ImmediateError(error),
                        };
                        let acceptance_marker = match write_durable_acceptance_marker() {
                            Ok(marker) => marker,
                            Err(_) => {
                                let _ = token.discard();
                                return CommandOutcome::ImmediateError(CommandError::new(
                                    "ACCEPTANCE_FAILED",
                                    "work was not accepted",
                                ));
                            }
                        };
                        let accepted_token = request.body.get("token").cloned().unwrap_or_default();
                        if let Err(error) = token.activate() {
                            remove_durable_acceptance_marker(&acceptance_marker);
                            let _ = token.discard();
                            return CommandOutcome::ImmediateError(CommandError::new(
                                "ACTIVATION_FAILED",
                                error.to_string(),
                            ));
                        }
                        let settlement = token.clone();
                        CommandOutcome::deferred_with_continuation(token, async move {
                            let settled = settlement
                                .settle_success(Some(json!({
                                    "token": accepted_token,
                                    "responder": LANG,
                                    "durablyAccepted": true
                                })))
                                .await
                                .map_err(|error| {
                                    CommandError::new("SETTLEMENT_FAILED", error.to_string())
                                });
                            remove_durable_acceptance_marker(&acceptance_marker);
                            settled
                        })
                    }),
                )
                .expect("register deferred command handler");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
        "deferred-request" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let reply_topic = format!(
                "interop/deferred/reply/{LANG}/{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock after epoch")
                    .as_nanos()
            );
            let svc = provider("deferredreq").await;
            let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
            let received = replies.clone();
            svc.subscribe(
                &reply_topic,
                message_handler(move |_topic, reply| {
                    let received = received.clone();
                    async move {
                        received.lock().unwrap().push(reply);
                    }
                }),
                16,
                2,
            )
            .await
            .expect("subscribe reply topic");
            let request = MessageBuilder::new("deferred", "1.0")
                .command(json!({ "token": token, "from": LANG }))
                .reply_to(&reply_topic)
                .build();
            let correlation = request.header.correlation_id.clone();
            svc.publish(&topic, &request)
                .await
                .expect("publish command");
            for _ in 0..80 {
                if !replies.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if replies.lock().unwrap().is_empty() {
                println!("{}", json!({"ok": false, "error": "timeout"}));
                std::process::exit(1);
            }
            // Retain the subscription after the first response so a double settlement fails.
            tokio::time::sleep(Duration::from_millis(750)).await;
            let replies = replies.lock().unwrap();
            let reply_count = replies.len();
            let reply = &replies[0];
            let correlation_match = reply.header.correlation_id == correlation;
            let result = reply.body.get("result");
            let ok = reply_count == 1
                && correlation_match
                && reply.body.get("ok").and_then(|value| value.as_bool()) == Some(true)
                && result
                    .and_then(|value| value.get("token"))
                    .and_then(|value| value.as_str())
                    == Some(token.as_str())
                && result
                    .and_then(|value| value.get("durablyAccepted"))
                    .and_then(|value| value.as_bool())
                    == Some(true)
                && result
                    .and_then(|value| value.get("responder"))
                    .and_then(|value| value.as_str())
                    .is_some();
            println!(
                "{}",
                json!({
                    "ok": ok,
                    "reply_count": reply_count,
                    "correlation_match": correlation_match,
                    "reply_body": reply.body
                })
            );
            std::process::exit(if ok { 0 } else { 1 });
        }
        // status-responder <component> — a real component that registers the canonical instance
        // connectivity provider. The runtime's built-in command inbox (started by build()) serves
        // the `status` verb from that same provider — the PULL surface.
        "status-responder" => {
            let component_token = args[2].clone();
            let path = write_command_runtime_config(&component_token);
            let gg = EdgeCommonsBuilder::new(format!(
                "com.mbreissi.edgecommons.interop.{LANG}.StatusResponder"
            ))
            .args(log_runtime_args(&path))
            .build()
            .await
            .expect("build status responder");
            let connectivity: Arc<InstanceConnectivityProvider> =
                Arc::new(interop_instance_connectivity);
            gg.set_instance_connectivity_provider(Some(connectivity));
            gg.commands().expect("runtime command inbox");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
        // status-request <component> — pull that component's built-in `status` verb over its own
        // command inbox (ecv1/interop-device/<component>/cmd/status) and print the verb's
        // result object (the inbox wraps it as {"ok":true,"result":{…}}).
        "status-request" => {
            let component_token = args[2].clone();
            let topic = interop_uns(&component_token)
                .topic_with_channel(UnsClass::Cmd, "status")
                .expect("mint the status command topic");
            let svc = provider("statusreq").await;
            let request = MessageBuilder::new("status", "1.0")
                .command(json!({ "from": LANG }))
                .build();
            let correlation = request.header.correlation_id.clone();
            let fut = match svc.request(&topic, request).await {
                Ok(fut) => fut,
                Err(error) => {
                    println!("{}", json!({"ok": false, "error": error.to_string()}));
                    std::process::exit(1);
                }
            };
            match tokio::time::timeout(Duration::from_secs(20), fut).await {
                Ok(Ok(reply)) => {
                    let correlation_match = reply.header.correlation_id == correlation;
                    let replied_ok =
                        reply.body.get("ok").and_then(|value| value.as_bool()) == Some(true);
                    let result = reply.body.get("result").cloned();
                    match result {
                        Some(result) if replied_ok && correlation_match => {
                            println!(
                                "{}",
                                json!({
                                    "ok": true,
                                    "correlation_match": correlation_match,
                                    "reply_body": result
                                })
                            );
                        }
                        _ => {
                            println!(
                                "{}",
                                json!({
                                    "ok": false,
                                    "error": format!(
                                        "unexpected status reply (correlation_match={correlation_match}): {}",
                                        reply.body
                                    )
                                })
                            );
                            std::process::exit(1);
                        }
                    }
                }
                Ok(Err(error)) => {
                    println!("{}", json!({"ok": false, "error": error.to_string()}));
                    std::process::exit(1);
                }
                Err(_) => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        // state-instances-pub <component> — the PUSH surface: the same component with the
        // heartbeat ENABLED, so the state keepalive carries the same provider sample in
        // instances[].
        "state-instances-pub" => {
            let component_token = args[2].clone();
            let path = write_state_runtime_config(&component_token);
            let gg = EdgeCommonsBuilder::new(format!(
                "com.mbreissi.edgecommons.interop.{LANG}.StatePublisher"
            ))
            .args(log_runtime_args(&path))
            .build()
            .await
            .expect("build state keepalive publisher");
            let connectivity: Arc<InstanceConnectivityProvider> =
                Arc::new(interop_instance_connectivity);
            gg.set_instance_connectivity_provider(Some(connectivity));
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
        // state-instances-sub <component> — subscribe that component's reserved `state` topic
        // (subscribing a reserved class is allowed; only PUBLISHING is rejected) and report the
        // first RUNNING keepalive that carries a non-empty instances[].
        "state-instances-sub" => {
            let component_token = args[2].clone();
            let topic = interop_uns(&component_token)
                .topic(UnsClass::State)
                .expect("mint the state topic");
            let svc = provider("statesub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        let running =
                            m.body.get("status").and_then(|v| v.as_str()) == Some("RUNNING");
                        let instances = m
                            .body
                            .get("instances")
                            .and_then(|v| v.as_array())
                            .filter(|instances| !instances.is_empty())
                            .cloned();
                        // The first RUNNING keepalive can precede the provider's registration;
                        // wait for one that actually carries the sample.
                        if let Some(instances) = instances {
                            if running {
                                let mut slot = rh.lock().unwrap();
                                if slot.is_none() {
                                    *slot = Some(instances);
                                }
                            }
                        }
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe the reserved state topic");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..400 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let observed = recv.lock().unwrap().take();
            match observed {
                Some(instances) => {
                    println!(
                        "{}",
                        json!({
                            "ok": true,
                            "state_status": "RUNNING",
                            "instances": instances
                        })
                    );
                }
                None => {
                    println!(
                        "{}",
                        json!({"ok": false, "error": "timeout waiting for a state with instances[]"})
                    );
                    std::process::exit(1);
                }
            }
        }
        "confirmed-sub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("confirmedsub").await;
            let messages = Arc::new(std::sync::Mutex::new(Vec::new()));
            let received = messages.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_topic, message| {
                    let received = received.clone();
                    async move {
                        received.lock().unwrap().push(message);
                    }
                }),
                16,
                2,
            )
            .await
            .expect("subscribe confirmed topic");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..80 {
                if !messages.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if messages.lock().unwrap().is_empty() {
                println!("{}", json!({"ok": false, "error": "timeout"}));
                std::process::exit(1);
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
            let messages = messages.lock().unwrap();
            let message_count = messages.len();
            let body = &messages[0].body;
            let ok = message_count == 1
                && body.get("token").and_then(|value| value.as_str()) == Some(token.as_str())
                && body.get("from").and_then(|value| value.as_str()).is_some();
            println!(
                "{}",
                json!({"ok": ok, "message_count": message_count, "body": body})
            );
            std::process::exit(if ok { 0 } else { 1 });
        }
        "confirmed-pub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("confirmedpub").await;
            let message = MessageBuilder::new("InteropConfirmed", "1.0")
                .payload(json!({ "token": token, "from": LANG }))
                .build();
            // The strict provider path resolves only after local MQTT PUBACK at QoS 1.
            match svc
                .publish_confirmed(&topic, &message, Duration::from_secs(5))
                .await
            {
                Ok(()) => println!("{}", json!({"ok": true, "confirmed": true, "qos": 1})),
                Err(error) => {
                    println!("{}", json!({"ok": false, "error": error.to_string()}));
                    std::process::exit(1);
                }
            }
        }
        "raw-sub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("rawsub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        *rh.lock().unwrap() = Some((m.is_raw(), m.get_raw().cloned()));
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().clone();
            match result {
                Some((is_raw, raw)) => {
                    println!(
                        "{}",
                        json!({
                            "ok": false,
                            "delivered": true,
                            "is_raw": is_raw,
                            "raw": raw,
                            "expected_token": token,
                        })
                    );
                    std::process::exit(1);
                }
                None => {
                    println!(
                        "{}",
                        json!({"ok": true, "delivered": false, "error": "timeout"})
                    );
                    std::process::exit(0);
                }
            }
        }
        "raw-pub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("rawpub").await;
            svc.publish_raw(&topic, &json!({ "token": token, "from": LANG }))
                .await
                .expect("publish_raw");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        "binary-sub" => {
            let topic = args[2].clone();
            let expected_hex = args[3].to_lowercase();
            let svc = provider("binsub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        let result = match (m.is_binary_body(), m.binary_body()) {
                            (is_binary, Ok(Some(bytes))) => {
                                json!({"is_binary": is_binary, "hex": encode_hex(&bytes)})
                            }
                            (is_binary, Ok(None)) => json!({"is_binary": is_binary, "hex": null}),
                            (is_binary, Err(e)) => {
                                json!({"is_binary": is_binary, "hex": null, "error": e.to_string()})
                            }
                        };
                        *rh.lock().unwrap() = Some(result);
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().take();
            match result {
                Some(mut payload) => {
                    let ok = payload
                        .get("is_binary")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        && payload.get("hex").and_then(|v| v.as_str())
                            == Some(expected_hex.as_str());
                    payload["ok"] = json!(ok);
                    println!("{}", payload);
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "binary-pub" => {
            let topic = args[2].clone();
            let bytes = decode_hex(&args[3]).expect("body hex");
            let svc = provider("binpub").await;
            let msg = MessageBuilder::new("InteropBinary", "1.0")
                .binary_payload(&bytes)
                .expect("binary payload")
                .tag("from", json!(LANG))
                .build();
            svc.publish(&topic, &msg).await.expect("publish");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        "typed-sub" => {
            let topic = args[2].clone();
            let expected_hex = args[3].to_lowercase();
            let svc = provider("typedsub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        let sample = &m.body["samples"][0];
                        let data = sample["value"]["_edgecommonsBinary"]["data"]
                            .as_str()
                            .and_then(|s| BASE64_STANDARD.decode(s).ok());
                        let result = json!({
                            "body_case": m.body_case().as_str(),
                            "hex": data.as_ref().map(|bytes| encode_hex(bytes)),
                            "source_ts_ms": sample["sourceTsMs"].as_u64(),
                            "server_ts_ms": sample["serverTsMs"].as_u64(),
                            "tag_from": m.tags.as_ref().and_then(|tags| tags.extra.get("from")).cloned()
                        });
                        *rh.lock().unwrap() = Some(result);
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().take();
            match result {
                Some(mut payload) => {
                    let ok = payload.get("body_case").and_then(|v| v.as_str())
                        == Some(MessageBodyCase::SouthboundSignalUpdate.as_str())
                        && payload.get("hex").and_then(|v| v.as_str())
                            == Some(expected_hex.as_str())
                        && payload.get("source_ts_ms").and_then(|v| v.as_u64())
                            == Some(1_783_360_799_900_u64)
                        && payload.get("server_ts_ms").and_then(|v| v.as_u64())
                            == Some(1_783_360_800_000_u64);
                    payload["ok"] = json!(ok);
                    println!("{}", payload);
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "typed-pub" => {
            let topic = args[2].clone();
            let bytes = decode_hex(&args[3]).expect("body hex");
            let svc = provider("typedpub").await;
            let msg = MessageBuilder::new("SouthboundSignalUpdate", "1.0")
                .southbound_signal_update(typed_body(&bytes))
                .tag("from", json!(LANG))
                .build();
            svc.publish(&topic, &msg).await.expect("publish");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        "log-sub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("logsub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            let expected_topic = topic.clone();
            svc.subscribe(
                &topic,
                message_handler(move |t, m| {
                    let rh = rh.clone();
                    let expected_topic = expected_topic.clone();
                    let token = token.clone();
                    async move {
                        let identity = m.identity.as_ref();
                        let fields = &m.body["fields"];
                        let ok = t == expected_topic
                            && m.body["schema"].as_str() == Some("edgecommons.log.v1")
                            && m.body["level"].as_str() == Some("WARN")
                            && m.body["message"].as_str()
                                == Some(format!("log-interop-{token}").as_str())
                            && fields["nonce"].as_str() == Some(token.as_str())
                            && identity.is_some_and(|id| {
                                id.device() == "interop-device"
                                    && id.component().starts_with("interop-log-")
                                    // Component scope (D-U28): the wire identity omits `instance`.
                                    && id.instance().is_none()
                            })
                            && m.header.name == "log"
                            && m.header.version == "1.0";
                        *rh.lock().unwrap() = Some(json!({
                            "ok": ok,
                            "topic": t,
                            "header": m.header,
                            "identity": m.identity,
                            "body": m.body
                        }));
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().take();
            match result {
                Some(payload) => {
                    let ok = payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    println!("{}", payload);
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "log-pub" => {
            let token = args[2].clone();
            let path = write_log_runtime_config();
            let args = log_runtime_args(&path);
            let gg = EdgeCommonsBuilder::new(format!(
                "com.mbreissi.edgecommons.interop.{LANG}.LogPublisher"
            ))
            .args(args)
            .build()
            .await
            .expect("build EdgeCommons log publisher");
            gg.logs()
                .publish(
                    LogRecord::builder(
                        LogLevel::Warn,
                        format!("interop.{LANG}"),
                        format!("log-interop-{token}"),
                    )
                    .field("nonce", json!(token))
                    .field("publisher", json!(LANG))
                    .build(),
                )
                .await
                .expect("publish log record");
            let stats = gg.logs().stats();
            let ok = stats.published >= 1;
            drop(gg);
            let _ = std::fs::remove_file(&path);
            println!(
                "{}",
                json!({
                    "ok": ok,
                    "component": log_component_token(),
                    "stats": {
                        "published": stats.published,
                        "failed": stats.failed,
                        "queued": stats.queued,
                        "dropped": stats.dropped
                    }
                })
            );
            std::process::exit(if ok { 0 } else { 1 });
        }
        "gg-log-matrix" => run_gg_log_matrix(&args).await,
        "gg-binary-matrix" => run_gg_binary_matrix(&args).await,
        "gg-p1-matrix" => run_gg_p1_matrix(&args).await,
        "gg-config-request" => run_gg_config_request(&args).await,
        "gg-config-update" => run_gg_config_update(&args).await,
        "gg-config-update-file" => run_gg_config_update_file(&args).await,
        "gg-command-request" => run_gg_command_request(&args).await,
        // uns-pub <identityJson> <class> [channel] — mint the topic with the real Uns
        // builder (includeRoot=false), stamp the identity via the real MessageBuilder,
        // publish, and print {"ok":true,"topic":...,"envelope":...}.
        "uns-pub" => {
            let identity_value: serde_json::Value =
                serde_json::from_str(&args[2]).expect("identity argument must be JSON");
            let Some(identity) = MessageIdentity::from_wire(&identity_value) else {
                eprintln!("bad identity: {}", args[2]);
                std::process::exit(2);
            };
            let Some(cls) = UnsClass::from_token(&args[3]) else {
                eprintln!("bad class: {}", args[3]);
                std::process::exit(2);
            };
            let channel = args.get(4).cloned();
            let uns = Uns::new(identity.clone(), false);
            let topic = match channel.as_deref() {
                Some(c) if !c.is_empty() => uns.topic_with_channel(cls, c),
                _ => uns.topic(cls),
            }
            .expect("mint UNS topic");
            let svc = provider("unspub").await;
            let msg = MessageBuilder::new("UnsInterop", "1.0")
                .payload(json!({ "from": LANG }))
                .identity(identity)
                .build();
            svc.publish(&topic, &msg).await.expect("publish");
            tokio::time::sleep(Duration::from_millis(500)).await;
            println!("{}", json!({ "ok": true, "topic": topic, "envelope": msg }));
        }
        // uns-sub <topic> — receive one envelope and print its parsed identity.
        "uns-sub" => {
            let topic = args[2].clone();
            let svc = provider("unssub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        *rh.lock().unwrap() = Some(m);
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().take();
            match result {
                Some(m) => {
                    let ok = m.identity.is_some();
                    println!(
                        "{}",
                        json!({ "ok": ok, "identity": m.identity, "body": m.body })
                    );
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        // uns-guard — attempt a raw publish to a reserved-class topic through the
        // guarded public service; must fail with EdgeCommonsError::ReservedTopic (§4.1).
        "uns-guard" => {
            let svc = provider("guard").await;
            // Reserved-class target selectable (D-U28): instance-scoped default or the
            // component-scoped ecv1/dev1/comp1/state — the guard must reject both.
            let topic = args.get(2).map(String::as_str).unwrap_or("ecv1/dev1/comp1/main/state");
            match svc.publish_raw(topic, &json!({ "from": LANG })).await {
                Err(EdgeCommonsError::ReservedTopic(detail)) => {
                    println!(
                        "{}",
                        json!({ "error": "ReservedTopic", "detail": detail, "topic": topic })
                    );
                    std::process::exit(3);
                }
                Err(e) => {
                    println!("{}", json!({ "error": format!("{e}") }));
                    std::process::exit(4);
                }
                Ok(()) => {
                    println!("{}", json!({ "ok": true }));
                }
            }
        }
        other => {
            eprintln!("unknown role: {other}");
            std::process::exit(2);
        }
    }
}
