//! Integration tests for the standalone MQTT messaging path.
//!
//! These exercise the real `rumqttc` provider against a live broker and are
//! therefore **gated**: they no-op unless `EDGECOMMONS_IT_MQTT=1` is set, so the
//! default `cargo test` run stays green on machines without a broker. They target
//! `localhost:1883` (override host/port via `EDGECOMMONS_IT_MQTT_HOST` /
//! `EDGECOMMONS_IT_MQTT_PORT`).
//!
//! Each test logs what it does (including message contents) via `tracing` to
//! **both** a file and the console:
//! - File: `target/test-logs/messaging_mqtt.log` (always written; truncated at the
//!   start of each test-process run). Review it after the run regardless of how the
//!   tests were launched.
//! - Console: shown only with `--nocapture` (cargo hides captured output for
//!   passing tests).
//!
//! ```bash
//! EDGECOMMONS_IT_MQTT=1 cargo test --test messaging_mqtt -- --nocapture --test-threads=1
//! ```
//! Adjust verbosity with `RUST_LOG` (default `info,edgecommons=debug`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use uuid::Uuid;

use edgecommons::messaging::config::MessagingConfig;
use edgecommons::messaging::message::{Message, MessageBuilder};
use edgecommons::messaging::message_handler;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::service::{DefaultMessagingService, MessagingService};

const MAX_MESSAGES: usize = 32;

/// Returns `true` (and prints a skip notice) when the broker-backed tests are disabled.
fn skipped() -> bool {
    if std::env::var("EDGECOMMONS_IT_MQTT").is_ok() {
        return false;
    }
    eprintln!("skipping MQTT integration test (set EDGECOMMONS_IT_MQTT=1 to enable)");
    true
}

/// Directory and file the integration-test logs are written to.
const LOG_DIR: &str = "target/test-logs";
const LOG_FILE: &str = "messaging_mqtt.log";

/// Install a tracing subscriber that writes to **both** the console (via the test
/// writer, shown with `--nocapture`) and a log file at
/// `target/test-logs/messaging_mqtt.log`. Safe to call from every test; the setup
/// runs exactly once per test-process.
fn init_logs() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = std::fs::create_dir_all(LOG_DIR);
        // Start each run with a fresh file.
        let _ = std::fs::remove_file(format!("{LOG_DIR}/{LOG_FILE}"));

        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,edgecommons=debug"));

        // Synchronous, non-rolling file appender (no background-flush guard to keep
        // alive) tee'd with the console test writer.
        let file = tracing_appender::rolling::never(LOG_DIR, LOG_FILE);
        let writer = tracing_subscriber::fmt::TestWriter::default().and(file);

        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false) // keep the file (and console) free of color escapes
            .with_writer(writer)
            .try_init();
    });
}

/// Render a message as compact JSON for logging.
fn as_json(m: &Message) -> String {
    serde_json::to_string(m).unwrap_or_else(|_| "<unserializable>".to_string())
}

/// Build a local-only messaging config pointing at the test broker.
fn local_config(client_id: &str) -> MessagingConfig {
    let host = std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let json = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "{client_id}" }} }} }}"#
    );
    serde_json::from_str(&json).expect("valid messaging config")
}

async fn connect_service(client_id: &str) -> Arc<DefaultMessagingService> {
    info!(client_id, "connecting to local broker");
    let mc = local_config(client_id);
    let provider = Arc::new(
        MqttProvider::connect(&mc)
            .await
            .expect("connect to local broker"),
    );
    info!(client_id, "connected");
    Arc::new(DefaultMessagingService::new(provider))
}

#[tokio::test]
async fn publish_subscribe_invokes_handler() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST publish_subscribe_invokes_handler ===");
    let svc = connect_service(&format!("it-pubsub-{}", Uuid::new_v4())).await;
    let topic = format!("itest/pubsub/{}", Uuid::new_v4());

    let received: Arc<Mutex<Option<(String, serde_json::Value)>>> = Arc::new(Mutex::new(None));
    let count = Arc::new(AtomicUsize::new(0));
    let (received_h, count_h) = (received.clone(), count.clone());

    info!(topic, max_messages = MAX_MESSAGES, max_concurrency = 1, "subscribing");
    svc.subscribe(
        &topic,
        message_handler(move |topic, msg| {
            let (received_h, count_h) = (received_h.clone(), count_h.clone());
            async move {
                info!(topic = %topic, message = %as_json(&msg), "handler received message");
                *received_h.lock().unwrap() = Some((topic, msg.body.clone()));
                count_h.fetch_add(1, Ordering::SeqCst);
            }
        }),
        MAX_MESSAGES,
        1,
    )
    .await
    .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await; // let SUBACK settle

    let msg = MessageBuilder::new("Evt", "1.0")
        .payload(json!({ "n": 1 }))
        .tag("origin", json!("test-thing"))
        .build();
    info!(topic, message = %as_json(&msg), "publishing message");
    svc.publish(&topic, &msg).await.expect("publish");

    for _ in 0..50 {
        if count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let got = received.lock().unwrap().clone().expect("handler was invoked");
    info!(topic = %got.0, body = %got.1, "asserting received body");
    assert_eq!(got.0, topic);
    assert_eq!(got.1["n"], 1);
    info!("=== PASS publish_subscribe_invokes_handler ===");
}

#[tokio::test]
async fn request_reply_roundtrip() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST request_reply_roundtrip ===");
    let svc = connect_service(&format!("it-reqrep-{}", Uuid::new_v4())).await;
    let req_topic = format!("itest/req/{}", Uuid::new_v4());

    // Responder: a callback subscription that replies to each request.
    let responder_svc = svc.clone();
    info!(req_topic, "responder subscribing");
    svc.subscribe(
        &req_topic,
        message_handler(move |topic, request| {
            let responder_svc = responder_svc.clone();
            async move {
                info!(topic = %topic, request = %as_json(&request), "responder received request");
                let reply = MessageBuilder::new("Pong", "1.0")
                    .payload(json!({ "ok": true }))
                    .tag("origin", json!("test-thing"))
                    .build();
                info!(reply_to = ?request.header.reply_to, reply = %as_json(&reply), "responder sending reply");
                if let Err(e) = responder_svc.reply(&request, reply).await {
                    info!(error = %e, "responder reply failed");
                }
            }
        }),
        MAX_MESSAGES,
        1,
    )
    .await
    .expect("responder subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let request = MessageBuilder::new("Ping", "1.0")
        .payload(json!({ "ping": 1 }))
        .tag("origin", json!("test-thing"))
        .build();
    let correlation = request.header.correlation_id.clone();

    info!(req_topic, request = %as_json(&request), "sending request");
    let reply_future = svc.request(&req_topic, request).await.expect("request issued");
    let reply = tokio::time::timeout(Duration::from_secs(5), reply_future)
        .await
        .expect("did not time out")
        .expect("a reply message");
    info!(reply = %as_json(&reply), correlation_id = %reply.header.correlation_id, "received reply");

    assert_eq!(reply.header.name, "Pong");
    assert_eq!(reply.body["ok"], true);
    assert_eq!(reply.header.correlation_id, correlation);
    info!("=== PASS request_reply_roundtrip ===");
}

#[tokio::test]
async fn serial_subscription_preserves_order() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST serial_subscription_preserves_order ===");
    let svc = connect_service(&format!("it-serial-{}", Uuid::new_v4())).await;
    let topic = format!("itest/serial/{}", Uuid::new_v4());

    let order = Arc::new(Mutex::new(Vec::<u64>::new()));
    let done = Arc::new(AtomicUsize::new(0));
    let (order_h, done_h) = (order.clone(), done.clone());

    info!(topic, max_concurrency = 1, "subscribing (serial)");
    svc.subscribe(
        &topic,
        message_handler(move |topic, m| {
            let (order_h, done_h) = (order_h.clone(), done_h.clone());
            async move {
                let n = m.body.as_u64().unwrap();
                info!(topic = %topic, n, message = %as_json(&m), "handler processing (dwelling)");
                // Earlier messages dwell longer; serial dispatch must still
                // record them in arrival order.
                tokio::time::sleep(Duration::from_millis((5 - n) * 20)).await;
                order_h.lock().unwrap().push(n);
                done_h.fetch_add(1, Ordering::SeqCst);
                info!(n, "handler finished");
            }
        }),
        MAX_MESSAGES,
        1, // serial
    )
    .await
    .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    for n in 0..4u64 {
        let m = MessageBuilder::new("Seq", "1.0").payload(json!(n)).build();
        info!(topic, n, message = %as_json(&m), "publishing message");
        svc.publish(&topic, &m).await.expect("publish");
    }

    for _ in 0..50 {
        if done.load(Ordering::SeqCst) >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let final_order = order.lock().unwrap().clone();
    info!(?final_order, "asserting processing order");
    assert_eq!(final_order, vec![0, 1, 2, 3]);
    info!("=== PASS serial_subscription_preserves_order ===");
}

#[tokio::test]
async fn request_times_out_with_no_responder() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST request_times_out_with_no_responder ===");
    let svc = connect_service(&format!("it-timeout-{}", Uuid::new_v4())).await;
    let topic = format!("itest/noone/{}", Uuid::new_v4());

    let request = MessageBuilder::new("Ping", "1.0").build();
    info!(topic, request = %as_json(&request), "sending request expecting timeout (no responder)");
    let reply_future = svc.request(&topic, request).await.expect("request issued");
    let result = tokio::time::timeout(Duration::from_millis(400), reply_future).await;

    info!(timed_out = result.is_err(), "request await completed");
    assert!(result.is_err(), "expected the await to time out, got {result:?}");
    info!("=== PASS request_times_out_with_no_responder ===");
}

#[tokio::test]
async fn publish_raw_is_received_as_raw() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST publish_raw_is_received_as_raw ===");
    let svc = connect_service(&format!("it-raw-{}", Uuid::new_v4())).await;
    let topic = format!("itest/raw/{}", Uuid::new_v4());

    let received = Arc::new(Mutex::new(None));
    let rh = received.clone();
    svc.subscribe(
        &topic,
        message_handler(move |_t, msg| {
            let rh = rh.clone();
            async move {
                *rh.lock().unwrap() = Some((msg.is_raw(), msg.get_raw().cloned()));
            }
        }),
        MAX_MESSAGES,
        1,
    )
    .await
    .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let payload = json!({ "sensor": "temp", "value": 21.5 });
    svc.publish_raw(&topic, &payload).await.expect("publish_raw");

    for _ in 0..50 {
        if received.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (is_raw, raw) = received.lock().unwrap().clone().expect("received a message");
    assert!(is_raw, "a non-envelope payload must be delivered as a raw message");
    assert_eq!(raw.expect("raw value"), payload);
    info!("=== PASS publish_raw_is_received_as_raw ===");
}
