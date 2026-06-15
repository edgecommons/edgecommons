//! Integration tests for the standalone MQTT messaging path.
//!
//! These exercise the real `rumqttc` provider against a live broker and are
//! therefore **gated**: they no-op unless `GGCOMMONS_IT_MQTT=1` is set, so the
//! default `cargo test` run stays green on machines without a broker. They target
//! `localhost:1883` (override host/port via `GGCOMMONS_IT_MQTT_HOST` /
//! `GGCOMMONS_IT_MQTT_PORT`).
//!
//! Run them with:
//! ```bash
//! GGCOMMONS_IT_MQTT=1 cargo test --test messaging_mqtt
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use ggcommons::messaging::config::MessagingConfig;
use ggcommons::messaging::message::MessageBuilder;
use ggcommons::messaging::provider::mqtt::MqttProvider;
use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};
use ggcommons::messaging::{message_handler, Destination};

/// Returns `true` (and prints a skip notice) when the broker-backed tests are disabled.
fn skipped() -> bool {
    if std::env::var("GGCOMMONS_IT_MQTT").is_ok() {
        return false;
    }
    eprintln!("skipping MQTT integration test (set GGCOMMONS_IT_MQTT=1 to enable)");
    true
}

/// Build a local-only messaging config pointing at the test broker.
fn local_config(client_id: &str) -> MessagingConfig {
    let host = std::env::var("GGCOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("GGCOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let json = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "{client_id}" }} }} }}"#
    );
    serde_json::from_str(&json).expect("valid messaging config")
}

async fn connect_service(client_id: &str) -> Arc<DefaultMessagingService> {
    let mc = local_config(client_id);
    let provider = Arc::new(
        MqttProvider::connect(&mc)
            .await
            .expect("connect to local broker"),
    );
    Arc::new(DefaultMessagingService::new(provider))
}

#[tokio::test]
async fn publish_subscribe_invokes_handler() {
    if skipped() {
        return;
    }
    let svc = connect_service(&format!("it-pubsub-{}", Uuid::new_v4())).await;
    let topic = format!("itest/pubsub/{}", Uuid::new_v4());

    let received: Arc<Mutex<Option<(String, serde_json::Value)>>> = Arc::new(Mutex::new(None));
    let count = Arc::new(AtomicUsize::new(0));
    let (received_h, count_h) = (received.clone(), count.clone());

    svc.subscribe(
        &topic,
        Destination::Local,
        1,
        message_handler(move |topic, msg| {
            let (received_h, count_h) = (received_h.clone(), count_h.clone());
            async move {
                *received_h.lock().unwrap() = Some((topic, msg.body.clone()));
                count_h.fetch_add(1, Ordering::SeqCst);
            }
        }),
    )
    .await
    .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await; // let SUBACK settle

    let msg = MessageBuilder::new("Evt", "1.0")
        .payload(json!({ "n": 1 }))
        .thing_name("test-thing")
        .build();
    svc.publish(&topic, &msg, Destination::Local)
        .await
        .expect("publish");

    // Wait for the callback to fire.
    for _ in 0..50 {
        if count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let got = received.lock().unwrap().clone().expect("handler was invoked");
    assert_eq!(got.0, topic);
    assert_eq!(got.1["n"], 1);
}

#[tokio::test]
async fn request_reply_roundtrip() {
    if skipped() {
        return;
    }
    let svc = connect_service(&format!("it-reqrep-{}", Uuid::new_v4())).await;
    let req_topic = format!("itest/req/{}", Uuid::new_v4());

    // Responder: a callback subscription that replies to each request.
    let responder_svc = svc.clone();
    svc.subscribe(
        &req_topic,
        Destination::Local,
        1,
        message_handler(move |_topic, request| {
            let responder_svc = responder_svc.clone();
            async move {
                let reply = MessageBuilder::new("Pong", "1.0")
                    .payload(json!({ "ok": true }))
                    .thing_name("test-thing")
                    .build();
                if let Err(e) = responder_svc.reply(&request, reply).await {
                    eprintln!("responder reply failed: {e}");
                }
            }
        }),
    )
    .await
    .expect("responder subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let request = MessageBuilder::new("Ping", "1.0")
        .payload(json!({ "ping": 1 }))
        .thing_name("test-thing")
        .build();
    let correlation = request.header.correlation_id.clone();

    let reply = svc
        .request(&req_topic, request, Destination::Local, Duration::from_secs(5))
        .await
        .expect("request returns a reply");

    assert_eq!(reply.header.name, "Pong");
    assert_eq!(reply.body["ok"], true);
    assert_eq!(reply.header.correlation_id, correlation);
}

#[tokio::test]
async fn serial_subscription_preserves_order() {
    if skipped() {
        return;
    }
    let svc = connect_service(&format!("it-serial-{}", Uuid::new_v4())).await;
    let topic = format!("itest/serial/{}", Uuid::new_v4());

    let order = Arc::new(Mutex::new(Vec::<u64>::new()));
    let done = Arc::new(AtomicUsize::new(0));
    let (order_h, done_h) = (order.clone(), done.clone());

    svc.subscribe(
        &topic,
        Destination::Local,
        1, // serial
        message_handler(move |_topic, m| {
            let (order_h, done_h) = (order_h.clone(), done_h.clone());
            async move {
                let n = m.body.as_u64().unwrap();
                // Earlier messages dwell longer; serial dispatch must still
                // record them in arrival order.
                tokio::time::sleep(Duration::from_millis((5 - n) * 20)).await;
                order_h.lock().unwrap().push(n);
                done_h.fetch_add(1, Ordering::SeqCst);
            }
        }),
    )
    .await
    .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    for n in 0..4u64 {
        let m = MessageBuilder::new("Seq", "1.0").payload(json!(n)).thing_name("t").build();
        svc.publish(&topic, &m, Destination::Local).await.expect("publish");
    }

    for _ in 0..50 {
        if done.load(Ordering::SeqCst) >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(*order.lock().unwrap(), vec![0, 1, 2, 3]);
}

#[tokio::test]
async fn request_times_out_with_no_responder() {
    if skipped() {
        return;
    }
    let svc = connect_service(&format!("it-timeout-{}", Uuid::new_v4())).await;
    let topic = format!("itest/noone/{}", Uuid::new_v4());

    let request = MessageBuilder::new("Ping", "1.0").thing_name("test-thing").build();
    let result = svc
        .request(&topic, request, Destination::Local, Duration::from_millis(400))
        .await;

    assert!(result.is_err(), "expected a timeout error, got {result:?}");
}
