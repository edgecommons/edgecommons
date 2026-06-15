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

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use ggcommons::messaging::config::MessagingConfig;
use ggcommons::messaging::message::MessageBuilder;
use ggcommons::messaging::provider::mqtt::MqttProvider;
use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};
use ggcommons::messaging::Destination;

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
async fn publish_subscribe_roundtrip() {
    if skipped() {
        return;
    }
    let svc = connect_service(&format!("it-pubsub-{}", Uuid::new_v4())).await;
    let topic = format!("itest/pubsub/{}", Uuid::new_v4());

    let mut stream = svc
        .subscribe(&topic, Destination::Local)
        .await
        .expect("subscribe");
    // Give the broker a moment to register the subscription (SUBACK).
    tokio::time::sleep(Duration::from_millis(300)).await;

    let msg = MessageBuilder::new("Evt", "1.0")
        .payload(json!({ "n": 1 }))
        .thing_name("test-thing")
        .build();
    svc.publish(&topic, &msg, Destination::Local)
        .await
        .expect("publish");

    let (recv_topic, received) = tokio::time::timeout(Duration::from_secs(5), stream.recv())
        .await
        .expect("did not time out")
        .expect("a message");
    assert_eq!(recv_topic, topic);
    assert_eq!(received.body["n"], 1);
    assert_eq!(received.header.name, "Evt");
}

#[tokio::test]
async fn request_reply_roundtrip() {
    if skipped() {
        return;
    }
    let svc = connect_service(&format!("it-reqrep-{}", Uuid::new_v4())).await;
    let req_topic = format!("itest/req/{}", Uuid::new_v4());

    // Responder: subscribe, signal readiness, reply to the first request.
    let responder_svc = svc.clone();
    let responder_topic = req_topic.clone();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let responder = tokio::spawn(async move {
        let mut stream = responder_svc
            .subscribe(&responder_topic, Destination::Local)
            .await
            .expect("responder subscribe");
        let _ = ready_tx.send(());
        if let Some((_t, request)) = stream.recv().await {
            let reply = MessageBuilder::new("Pong", "1.0")
                .payload(json!({ "ok": true }))
                .thing_name("test-thing")
                .build();
            responder_svc.reply(&request, reply).await.expect("reply");
        }
    });

    ready_rx.await.expect("responder ready");
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
    // The reply must carry the request's correlation id.
    assert_eq!(reply.header.correlation_id, correlation);

    responder.await.expect("responder task");
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
