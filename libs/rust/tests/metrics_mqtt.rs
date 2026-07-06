//! Integration test for the `messaging` metric target against a live broker
//! (UNS-CANONICAL-DESIGN §4.3): the full runtime publishes each metric to the
//! library-owned UNS topic `ecv1/{device}/{component}/main/metric/{metricName}`
//! through the privileged reserved-publish seam.
//!
//! Gated: no-op unless `EDGECOMMONS_IT_MQTT=1` is set. See `messaging_mqtt.rs` for the
//! broker/env conventions. Logs go to console (`--nocapture`) and to
//! `target/test-logs/metrics_mqtt.log`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use uuid::Uuid;

use edgecommons::messaging::config::MessagingConfig;
use edgecommons::messaging::message::Message;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::{Destination, MessagingProvider, Qos};
use edgecommons::prelude::*;

fn skipped() -> bool {
    if std::env::var("EDGECOMMONS_IT_MQTT").is_ok() {
        return false;
    }
    eprintln!("skipping MQTT integration test (set EDGECOMMONS_IT_MQTT=1 to enable)");
    true
}

fn init_logs() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = std::fs::create_dir_all("target/test-logs");
        let _ = std::fs::remove_file("target/test-logs/metrics_mqtt.log");
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,edgecommons=debug"));
        let file = tracing_appender::rolling::never("target/test-logs", "metrics_mqtt.log");
        let writer = tracing_subscriber::fmt::TestWriter::default().and(file);
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(writer)
            .try_init();
    });
}

fn broker() -> (String, String) {
    let host =
        std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    (host, port)
}

fn messaging_config(client_id: &str) -> MessagingConfig {
    let (host, port) = broker();
    let json = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "{client_id}" }} }} }}"#
    );
    serde_json::from_str(&json).expect("valid messaging config")
}

#[tokio::test]
async fn messaging_metric_target_publishes_emf_on_the_uns_topic() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST messaging_metric_target_publishes_emf_on_the_uns_topic ===");

    // A unique thing name isolates this run's UNS topics on the shared broker.
    let thing = format!("metric-thing-{}", Uuid::new_v4());
    let metric_topic = format!("ecv1/{thing}/MetricIt/main/metric/requests");

    // Raw observer subscription so we can read the published envelope directly.
    let mc = messaging_config(&format!("it-metric-obs-{}", Uuid::new_v4()));
    let observer = Arc::new(MqttProvider::connect(&mc).await.expect("connect"));
    info!(topic = %metric_topic, "subscribing (raw) to the UNS metric topic");
    let mut raw_sub = observer
        .subscribe(&metric_topic, Destination::Local, Qos::AtLeastOnce, 16)
        .await
        .expect("raw subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Full runtime configured for the messaging metric target.
    let dir = std::env::temp_dir().join(format!("edgecommons-metric-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let messaging_path = dir.join("messaging.json");
    let (host, port) = broker();
    std::fs::write(
        &config_path,
        json!({
            "heartbeat": { "enabled": false },
            "metricEmission": {
                "target": "messaging",
                "namespace": "demo",
                "targetConfig": { "destination": "ipc" }
            },
            "tags": { "site": "factory-1" },
            "component": { "global": {} }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        &messaging_path,
        format!(
            r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "it-metric-{}" }} }} }}"#,
            Uuid::new_v4()
        ),
    )
    .unwrap();

    let gg = EdgeCommonsBuilder::new("com.example.MetricIt")
        .args([
            "prog".to_string(),
            "--platform".to_string(),
            "HOST".to_string(),
            "--transport".to_string(),
            "MQTT".to_string(),
            messaging_path.to_string_lossy().into_owned(),
            "-c".to_string(),
            "FILE".to_string(),
            config_path.to_string_lossy().into_owned(),
            "-t".to_string(),
            thing.clone(),
        ])
        .build()
        .await
        .expect("build runtime");

    let metrics = gg.metrics();
    metrics.define_metric(
        MetricBuilder::create("requests")
            .with_config(&gg.config())
            .add_measure("count", "Count", 60)
            .build(),
    );

    let mut values = HashMap::new();
    values.insert("count".to_string(), 5.0);
    info!(topic = %metric_topic, "emitting metric");
    metrics
        .emit_metric_now("requests", values)
        .await
        .expect("emit");

    let (recv_topic, bytes) = tokio::time::timeout(Duration::from_secs(5), raw_sub.recv())
        .await
        .expect("did not time out")
        .expect("a message");
    let envelope = Message::from_slice(&bytes).expect("valid envelope");
    info!(topic = %recv_topic, "received metric envelope");

    assert_eq!(
        recv_topic, metric_topic,
        "metric lands on the UNS metric topic"
    );
    assert_eq!(envelope.header.name, "Metric");
    assert_eq!(envelope.header.version, "1.0");
    // The EMF object travels in the envelope BODY.
    let emf = &envelope.body;
    assert_eq!(emf["count"], 5.0);
    assert_eq!(emf["category"], "requests");
    assert_eq!(emf["coreName"], thing.as_str());
    assert_eq!(emf["_aws"]["CloudWatchMetrics"][0]["Namespace"], "demo");
    assert!(emf["_aws"]["Timestamp"].as_u64().unwrap() > 1_000_000_000_000);
    // Identity + tags are stamped from the config.
    let identity = envelope.identity.as_ref().expect("identity stamped");
    assert_eq!(identity.device(), thing);
    assert_eq!(identity.component(), "MetricIt");
    let tags = envelope.tags.as_ref().expect("tags stamped");
    assert_eq!(tags.extra.get("site"), Some(&json!("factory-1")));

    drop(gg);
    let _ = std::fs::remove_dir_all(&dir);
    info!("=== PASS messaging_metric_target_publishes_emf_on_the_uns_topic ===");
}
