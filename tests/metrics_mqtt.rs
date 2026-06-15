//! Integration test for the `messaging` metric target against a live broker.
//!
//! Gated: no-op unless `GGCOMMONS_IT_MQTT=1` is set. See `messaging_mqtt.rs` for the
//! broker/env conventions. Logs go to console (`--nocapture`) and to
//! `target/test-logs/metrics_mqtt.log`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use uuid::Uuid;

use ggcommons::config::model::Config;
use ggcommons::messaging::config::MessagingConfig;
use ggcommons::messaging::provider::mqtt::MqttProvider;
use ggcommons::messaging::service::DefaultMessagingService;
use ggcommons::messaging::{Destination, MessagingProvider, Qos};
use ggcommons::metrics::{MetricBuilder, MetricEmitter, MetricService};

fn skipped() -> bool {
    if std::env::var("GGCOMMONS_IT_MQTT").is_ok() {
        return false;
    }
    eprintln!("skipping MQTT integration test (set GGCOMMONS_IT_MQTT=1 to enable)");
    true
}

fn init_logs() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = std::fs::create_dir_all("target/test-logs");
        let _ = std::fs::remove_file("target/test-logs/metrics_mqtt.log");
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ggcommons=debug"));
        let file = tracing_appender::rolling::never("target/test-logs", "metrics_mqtt.log");
        let writer = tracing_subscriber::fmt::TestWriter::default().and(file);
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(writer)
            .try_init();
    });
}

fn messaging_config(client_id: &str) -> MessagingConfig {
    let host = std::env::var("GGCOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("GGCOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let json = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "{client_id}" }} }} }}"#
    );
    serde_json::from_str(&json).expect("valid messaging config")
}

#[tokio::test]
async fn messaging_metric_target_publishes_emf() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST messaging_metric_target_publishes_emf ===");

    let topic = format!("itest/metric/{}", Uuid::new_v4());
    let mc = messaging_config(&format!("it-metric-{}", Uuid::new_v4()));
    let provider = Arc::new(MqttProvider::connect(&mc).await.expect("connect"));

    // Raw subscription so we can read the published EMF JSON directly.
    info!(topic, "subscribing (raw) to the metric topic");
    let mut raw_sub = provider
        .subscribe(&topic, Destination::Local, Qos::AtLeastOnce, 16)
        .await
        .expect("raw subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Metric emitter configured to publish to the messaging target on that topic.
    let svc = Arc::new(DefaultMessagingService::new(provider.clone()));
    let raw = json!({
        "metricEmission": {
            "target": "messaging",
            "namespace": "demo",
            "targetConfig": { "topic": topic, "destination": "ipc" }
        }
    });
    let config = Config::from_value("com.example.C", "thing-1", raw).unwrap();
    let emitter = MetricEmitter::new(&config, Some(svc)).await.expect("emitter");

    emitter.define_metric(
        MetricBuilder::create("requests")
            .with_config(&config)
            .add_measure("count", "Count", 60)
            .build(),
    );

    let mut values = HashMap::new();
    values.insert("count".to_string(), 5.0);
    info!(topic, "emitting metric");
    emitter.emit_metric_now("requests", values).await.expect("emit");

    let (recv_topic, bytes) = tokio::time::timeout(Duration::from_secs(5), raw_sub.recv())
        .await
        .expect("did not time out")
        .expect("a message");
    let emf: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
    info!(topic = %recv_topic, emf = %emf, "received EMF");

    assert_eq!(recv_topic, topic);
    assert_eq!(emf["count"], 5.0);
    assert_eq!(emf["category"], "requests");
    assert_eq!(emf["coreName"], "thing-1");
    assert_eq!(emf["_aws"]["CloudWatchMetrics"][0]["Namespace"], "demo");
    assert!(emf["_aws"]["Timestamp"].as_u64().unwrap() > 1_000_000_000_000);
    info!("=== PASS messaging_metric_target_publishes_emf ===");
}
