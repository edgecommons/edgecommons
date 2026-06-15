//! Integration test for the heartbeat `messaging` target against a live broker.
//!
//! Gated: no-op unless `GGCOMMONS_IT_MQTT=1` is set. Logs go to console
//! (`--nocapture`) and to `target/test-logs/heartbeat_mqtt.log`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use uuid::Uuid;

use ggcommons::config::model::Config;
use ggcommons::heartbeat::Heartbeat;
use ggcommons::messaging::config::MessagingConfig;
use ggcommons::messaging::message::Message;
use ggcommons::messaging::message_handler;
use ggcommons::messaging::provider::mqtt::MqttProvider;
use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};
use ggcommons::metrics::{MetricEmitter, MetricService};

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
        let _ = std::fs::remove_file("target/test-logs/heartbeat_mqtt.log");
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ggcommons=debug"));
        let file = tracing_appender::rolling::never("target/test-logs", "heartbeat_mqtt.log");
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
async fn heartbeat_publishes_to_messaging_target() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST heartbeat_publishes_to_messaging_target ===");

    let topic = format!("itest/heartbeat/{}", Uuid::new_v4());
    let mc = messaging_config(&format!("it-hb-{}", Uuid::new_v4()));
    let provider = Arc::new(MqttProvider::connect(&mc).await.expect("connect"));
    let svc = Arc::new(DefaultMessagingService::new(provider));

    // Capture heartbeat messages arriving on the topic.
    let received: Arc<Mutex<Option<Message>>> = Arc::new(Mutex::new(None));
    let count = Arc::new(AtomicUsize::new(0));
    let (received_h, count_h) = (received.clone(), count.clone());
    info!(topic, "subscribing to heartbeat topic");
    svc.subscribe(
        &topic,
        message_handler(move |t, msg| {
            let (received_h, count_h) = (received_h.clone(), count_h.clone());
            async move {
                info!(topic = %t, name = %msg.header.name, "received heartbeat message");
                *received_h.lock().unwrap() = Some(msg);
                count_h.fetch_add(1, Ordering::SeqCst);
            }
        }),
        16,
        1,
    )
    .await
    .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Config: 1s heartbeat to the messaging target on our topic. Metrics target is a
    // temp log file (the heartbeat defines its metric regardless of targets).
    let metric_log = std::env::temp_dir().join(format!("ggcommons-hb-{}.log", Uuid::new_v4()));
    let raw = json!({
        "heartbeat": {
            "intervalSecs": 1,
            "measures": { "cpu": true, "memory": true },
            "targets": [ { "type": "messaging", "config": { "topic": topic, "destination": "ipc" } } ]
        },
        "metricEmission": { "target": "log", "targetConfig": { "logFileName": metric_log.to_string_lossy() } }
    });
    let config = Config::from_value("com.example.C", "thing-1", raw).unwrap();
    let metrics: Arc<dyn MetricService> =
        Arc::new(MetricEmitter::new(&config, Some(svc.clone())).await.expect("metrics"));

    info!("starting heartbeat");
    let _heartbeat = Heartbeat::start(&config, metrics, Some(svc.clone()));

    // First tick is immediate; wait for a heartbeat to arrive.
    for _ in 0..50 {
        if count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let msg = received.lock().unwrap().clone().expect("a heartbeat message arrived");
    info!(payload = %msg.body, "asserting heartbeat payload");
    assert_eq!(msg.header.name, "heartbeat");
    assert_eq!(msg.header.version, "1.0.0");
    assert!(msg.body["memory"]["memory_usage"].as_f64().unwrap() > 0.0);
    assert!(msg.body["cpu"]["cpu_usage"].is_number());

    let _ = std::fs::remove_file(&metric_log);
    info!("=== PASS heartbeat_publishes_to_messaging_target ===");
}
