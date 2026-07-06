//! Integration test for the combined STANDALONE dual-broker scenario: a single
//! provider connected to BOTH a local broker AND IoT Core at the same time, using
//! both transports.
//!
//! To run without real AWS, the `iotCore` endpoint is pointed at the SAME shared
//! EMQX as `local` but over the mutual-TLS listener (:8883), so the real
//! dual-client / dual-transport code path runs end-to-end. Because both point at
//! one broker, this validates that both connections are live and both method sets
//! work; distinct topics are used per transport (true cross-broker isolation would
//! need two separate brokers).
//!
//! Gated: requires `EDGECOMMONS_IT_MQTT=1` plus the mutual-TLS material for IoT Core
//! — `EDGECOMMONS_IT_MQTT_CA`, `EDGECOMMONS_IT_MQTT_CERT`, `EDGECOMMONS_IT_MQTT_KEY`
//! (IoT Core always requires mutual TLS). No-ops otherwise.
//!
//! ```bash
//! EDGECOMMONS_IT_MQTT=1 \
//!   EDGECOMMONS_IT_MQTT_CA=<infra>/tls-certs/ca.crt \
//!   EDGECOMMONS_IT_MQTT_CERT=<infra>/tls-certs/client.crt \
//!   EDGECOMMONS_IT_MQTT_KEY=<infra>/tls-certs/client.key \
//!   cargo test --test dual_broker_mqtt -- --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use edgecommons::messaging::config::MessagingConfig;
use edgecommons::messaging::message::MessageBuilder;
use edgecommons::messaging::message_handler;
use edgecommons::messaging::Qos;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::service::{DefaultMessagingService, MessagingService};

/// Returns (ca, cert, key) when the dual-broker scenario can run, else None.
fn creds_or_skip() -> Option<(String, String, String)> {
    if std::env::var("EDGECOMMONS_IT_MQTT").is_err() {
        eprintln!("skipping dual-broker test (set EDGECOMMONS_IT_MQTT=1)");
        return None;
    }
    let ca = std::env::var("EDGECOMMONS_IT_MQTT_CA").ok();
    let cert = std::env::var("EDGECOMMONS_IT_MQTT_CERT").ok();
    let key = std::env::var("EDGECOMMONS_IT_MQTT_KEY").ok();
    match (ca, cert, key) {
        (Some(ca), Some(cert), Some(key)) => Some((ca, cert, key)),
        _ => {
            eprintln!(
                "skipping dual-broker test (IoT Core needs mutual TLS: set \
                 EDGECOMMONS_IT_MQTT_CA/CERT/KEY)"
            );
            None
        }
    }
}

async fn connect_dual() -> Option<Arc<DefaultMessagingService>> {
    let (ca, cert, key) = creds_or_skip()?;
    let host = std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let local_port: u16 = std::env::var("EDGECOMMONS_IT_MQTT_PORT")
        .unwrap_or_else(|_| "1883".to_string())
        .parse()
        .unwrap();
    let tls_port: u16 = std::env::var("EDGECOMMONS_IT_MQTT_TLS_PORT")
        .unwrap_or_else(|_| "8883".to_string())
        .parse()
        .unwrap();

    let mc: MessagingConfig = serde_json::from_value(json!({
        "messaging": {
            "local": {
                "host": host,
                "port": local_port,
                "clientId": format!("it-dual-local-{}", Uuid::new_v4()),
            },
            "iotCore": {
                "endpoint": host,
                "port": tls_port,
                "clientId": format!("it-dual-iot-{}", Uuid::new_v4()),
                "credentials": { "caPath": ca, "certPath": cert, "keyPath": key },
            }
        }
    }))
    .expect("valid dual-broker config");

    // The parsed config must carry BOTH brokers.
    assert!(mc.messaging.iot_core.is_some(), "iotCore must be configured");

    let provider = Arc::new(MqttProvider::connect(&mc).await.expect("dual-broker connect"));
    Some(Arc::new(DefaultMessagingService::new(provider)))
}

fn msg(name: &str, payload: serde_json::Value) -> edgecommons::messaging::message::Message {
    MessageBuilder::new(name, "1.0").payload(payload).tag("origin", serde_json::json!("dual-thing")).build()
}

#[tokio::test]
async fn both_transports_deliver_simultaneously() {
    let Some(svc) = connect_dual().await else { return };

    let local_topic = format!("itest/dual/local/{}", Uuid::new_v4());
    let iot_topic = format!("itest/dual/iot/{}", Uuid::new_v4());

    let local_rx = Arc::new(std::sync::Mutex::new(None));
    let iot_rx = Arc::new(std::sync::Mutex::new(None));

    let lh = local_rx.clone();
    svc.subscribe(
        &local_topic,
        message_handler(move |_t, m| {
            let lh = lh.clone();
            async move { *lh.lock().unwrap() = Some(m.body.clone()); }
        }),
        16,
        1,
    )
    .await
    .expect("subscribe local");

    let ih = iot_rx.clone();
    svc.subscribe_to_iot_core(
        &iot_topic,
        message_handler(move |_t, m| {
            let ih = ih.clone();
            async move { *ih.lock().unwrap() = Some(m.body.clone()); }
        }),
        Qos::AtLeastOnce,
        16,
        1,
    )
    .await
    .expect("subscribe iot core");
    tokio::time::sleep(Duration::from_millis(300)).await;

    svc.publish(&local_topic, &msg("LocalMsg", json!({ "via": "local" })))
        .await
        .expect("publish local");
    svc.publish_to_iot_core(&iot_topic, &msg("IotMsg", json!({ "via": "iot" })), Qos::AtLeastOnce)
        .await
        .expect("publish iot core");

    for _ in 0..50 {
        if local_rx.lock().unwrap().is_some() && iot_rx.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let local_body = local_rx.lock().unwrap().clone().expect("local message delivered");
    let iot_body = iot_rx.lock().unwrap().clone().expect("iot core message delivered");
    assert_eq!(local_body["via"], "local");
    assert_eq!(iot_body["via"], "iot");

    svc.unsubscribe(&local_topic).await.ok();
    svc.unsubscribe_from_iot_core(&iot_topic).await.ok();
}

#[tokio::test]
async fn request_reply_on_both_transports() {
    let Some(svc) = connect_dual().await else { return };

    // Local request/reply.
    let local_req = format!("itest/dual/local/req/{}", Uuid::new_v4());
    let responder = svc.clone();
    svc.subscribe(
        &local_req,
        message_handler(move |_t, request| {
            let responder = responder.clone();
            async move {
                let _ = responder.reply(&request, msg("LReply", json!({ "answer": "local" }))).await;
            }
        }),
        16,
        1,
    )
    .await
    .expect("subscribe local req");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let local_request = msg("LReq", json!({ "q": 1 }));
    let local_corr = local_request.header.correlation_id.clone();
    let local_future = svc.request(&local_req, local_request).await.expect("local request");
    let local_reply = tokio::time::timeout(Duration::from_secs(5), local_future)
        .await
        .expect("local reply not timed out")
        .expect("local reply message");
    assert_eq!(local_reply.header.name, "LReply");
    assert_eq!(local_reply.header.correlation_id, local_corr);

    // IoT Core request/reply.
    let iot_req = format!("itest/dual/iot/req/{}", Uuid::new_v4());
    let responder2 = svc.clone();
    svc.subscribe_to_iot_core(
        &iot_req,
        message_handler(move |_t, request| {
            let responder2 = responder2.clone();
            async move {
                let _ = responder2
                    .reply_to_iot_core(&request, msg("IReply", json!({ "answer": "iot" })))
                    .await;
            }
        }),
        Qos::AtLeastOnce,
        16,
        1,
    )
    .await
    .expect("subscribe iot req");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let iot_request = msg("IReq", json!({ "q": 2 }));
    let iot_corr = iot_request.header.correlation_id.clone();
    let iot_future = svc.request_from_iot_core(&iot_req, iot_request).await.expect("iot request");
    let iot_reply = tokio::time::timeout(Duration::from_secs(5), iot_future)
        .await
        .expect("iot reply not timed out")
        .expect("iot reply message");
    assert_eq!(iot_reply.header.name, "IReply");
    assert_eq!(iot_reply.header.correlation_id, iot_corr);

    svc.unsubscribe(&local_req).await.ok();
    svc.unsubscribe_from_iot_core(&iot_req).await.ok();
}
