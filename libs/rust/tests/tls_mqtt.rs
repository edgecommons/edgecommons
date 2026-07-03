//! Integration test for the TLS transport against a live broker.
//!
//! Doubly gated: requires `GGCOMMONS_IT_MQTT=1` **and** `GGCOMMONS_IT_MQTT_CA`
//! pointing to a CA PEM that the broker's server certificate chains to (e.g. the
//! EMQX `cacert.pem` for its `8883` listener). Optionally set
//! `GGCOMMONS_IT_MQTT_CERT` / `GGCOMMONS_IT_MQTT_KEY` for mutual TLS. Without the CA
//! it no-ops, because TLS verification needs a trust anchor (we never disable it).
//!
//! Run, e.g.:
//! ```bash
//! GGCOMMONS_IT_MQTT=1 GGCOMMONS_IT_MQTT_CA=/path/to/emqx/cacert.pem \
//!   cargo test --test tls_mqtt -- --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use ggcommons::messaging::config::MessagingConfig;
use ggcommons::messaging::message::MessageBuilder;
use ggcommons::messaging::message_handler;
use ggcommons::messaging::provider::mqtt::MqttProvider;
use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};

fn skipped() -> Option<String> {
    if std::env::var("GGCOMMONS_IT_MQTT").is_err() {
        eprintln!("skipping TLS test (set GGCOMMONS_IT_MQTT=1)");
        return None;
    }
    match std::env::var("GGCOMMONS_IT_MQTT_CA") {
        Ok(ca) => Some(ca),
        Err(_) => {
            eprintln!("skipping TLS test (set GGCOMMONS_IT_MQTT_CA to the broker's CA PEM)");
            None
        }
    }
}

#[tokio::test]
async fn tls_publish_subscribe_roundtrip() {
    let Some(ca_path) = skipped() else { return };

    let host = std::env::var("GGCOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("GGCOMMONS_IT_MQTT_TLS_PORT").unwrap_or_else(|_| "8883".to_string());
    let cert = std::env::var("GGCOMMONS_IT_MQTT_CERT").ok();
    let key = std::env::var("GGCOMMONS_IT_MQTT_KEY").ok();

    // Build a local-broker config that uses TLS (caPath present => TLS; + cert/key => mTLS).
    let mut local = json!({
        "host": host,
        "port": port.parse::<u16>().unwrap(),
        "clientId": format!("it-tls-{}", Uuid::new_v4()),
        "credentials": { "caPath": ca_path }
    });
    if let (Some(cert), Some(key)) = (cert, key) {
        local["credentials"]["certPath"] = json!(cert);
        local["credentials"]["keyPath"] = json!(key);
    }
    let mc: MessagingConfig =
        serde_json::from_value(json!({ "messaging": { "local": local } })).unwrap();

    let provider = Arc::new(MqttProvider::connect(&mc).await.expect("TLS connect"));
    let svc = Arc::new(DefaultMessagingService::new(provider));

    let topic = format!("itest/tls/{}", Uuid::new_v4());
    let received = Arc::new(std::sync::Mutex::new(None));
    let received_h = received.clone();
    svc.subscribe(
        &topic,
        message_handler(move |_t, msg| {
            let received_h = received_h.clone();
            async move {
                *received_h.lock().unwrap() = Some(msg.body.clone());
            }
        }),
        16,
        1,
    )
    .await
    .expect("subscribe over TLS");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let msg = MessageBuilder::new("Evt", "1.0").payload(json!({ "n": 7 })).build();
    svc.publish(&topic, &msg).await.expect("publish over TLS");

    for _ in 0..50 {
        if received.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let body = received.lock().unwrap().clone().expect("received a message over TLS");
    assert_eq!(body["n"], 7);
}
