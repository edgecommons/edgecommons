//! Cross-language interop node (Rust) for edgecommons. See python_node.py for the
//! shared CLI contract:
//!   interop-rust-node responder <request_topic>
//!   interop-rust-node request   <request_topic> <token>
//!   interop-rust-node uns-pub   <identityJson> <class> [channel]
//!   interop-rust-node uns-sub   <topic>
//!   interop-rust-node uns-guard
//!   interop-rust-node gg-config-request <topic> <component> <output-json>
//!   interop-rust-node gg-config-update <topic> <output-json>
//! Local-only MQTT transport against localhost:1883. Messages are built without a
//! config — the envelope legally omits `identity` unless one is stamped explicitly
//! (the UNS roles); `tags.thing` no longer exists (UNS hard cut).

use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use edgecommons::prelude::{EdgeCommonsBuilder, LogLevel, LogRecord};
use serde_json::json;
#[cfg(feature = "greengrass")]
use serde_json::{Map, Value};

use edgecommons::error::EdgeCommonsError;
use edgecommons::messaging::config::MessagingConfig;
#[cfg(feature = "greengrass")]
use edgecommons::messaging::message::Message;
use edgecommons::messaging::message::{
    binary_value, MessageBodyCase, MessageBuilder, MessageIdentity,
};
use edgecommons::messaging::message_handler;
#[cfg(feature = "greengrass")]
use edgecommons::messaging::provider::ipc::IpcProvider;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::service::{DefaultMessagingService, MessagingService};
use edgecommons::uns::{Uns, UnsClass};

const LANG: &str = "rust";

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
        "interop-device".to_string(),
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
        "ecv1/interop-device/+/main/log/warn",
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
                        id.device() == "interop-device" && id.instance() == "main"
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
            let (lineage_ok, lineage_check) = validate_lineage_bundle(&component, &reply.body, None);
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
        let push_topic = format!("ecv1/lab-5950x/{token}/main/cmd/set-config");
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
        eprintln!("gg-config-update-file requires <topic> <catalog-json> <tokens-csv> <output-json>");
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
        let push_topic = format!("ecv1/lab-5950x/{token}/main/cmd/set-config");
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
        && result
            .get("correlation_match")
            .and_then(Value::as_bool)
            == Some(true);
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
                                    && id.instance() == "main"
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
            let topic = "ecv1/dev1/comp1/main/state";
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
