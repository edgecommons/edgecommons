//! Integration test for the heartbeat UNS `state` keepalive against a live broker
//! (UNS-CANONICAL-DESIGN §4.3, D-U14): the full runtime publishes
//! `ecv1/{device}/{component}/main/state` each tick through the privileged
//! reserved-publish seam.
//!
//! Gated: no-op unless `EDGECOMMONS_IT_MQTT=1` is set. Logs go to console
//! (`--nocapture`) and to `target/test-logs/heartbeat_mqtt.log`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tracing::info;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use uuid::Uuid;

use edgecommons::messaging::config::MessagingConfig;
use edgecommons::messaging::message::Message;
use edgecommons::messaging::message_handler;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::service::{DefaultMessagingService, MessagingService};
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
        let _ = std::fs::remove_file("target/test-logs/heartbeat_mqtt.log");
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,edgecommons=debug"));
        let file = tracing_appender::rolling::never("target/test-logs", "heartbeat_mqtt.log");
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
async fn heartbeat_publishes_uns_state_keepalive() {
    if skipped() {
        return;
    }
    init_logs();
    info!("=== TEST heartbeat_publishes_uns_state_keepalive ===");

    // A unique thing name isolates this run's UNS topics on the shared broker.
    let thing = format!("hb-thing-{}", Uuid::new_v4());
    let state_topic = format!("ecv1/{thing}/HbIt/main/state");

    // Observer client on the state topic.
    let mc = messaging_config(&format!("it-hb-obs-{}", Uuid::new_v4()));
    let provider = Arc::new(MqttProvider::connect(&mc).await.expect("connect"));
    let observer = Arc::new(DefaultMessagingService::new(provider));
    let received: Arc<Mutex<Vec<Message>>> = Arc::new(Mutex::new(Vec::new()));
    let count = Arc::new(AtomicUsize::new(0));
    let (received_h, count_h) = (received.clone(), count.clone());
    info!(topic = %state_topic, "subscribing to the UNS state topic");
    observer
        .subscribe(
            &state_topic,
            message_handler(move |t, msg| {
                let (received_h, count_h) = (received_h.clone(), count_h.clone());
                async move {
                    info!(topic = %t, name = %msg.header.name, "received state keepalive");
                    received_h.lock().unwrap().push(msg);
                    count_h.fetch_add(1, Ordering::SeqCst);
                }
            }),
            16,
            1,
        )
        .await
        .expect("subscribe");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Full runtime: 1s heartbeat; metrics to a temp log file.
    let dir = std::env::temp_dir().join(format!("edgecommons-hb-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let messaging_path = dir.join("messaging.json");
    let metric_log = dir.join("metric.log");
    let (host, port) = broker();
    std::fs::write(
        &config_path,
        json!({
            "heartbeat": { "intervalSecs": 1, "measures": { "cpu": true, "memory": true } },
            "metricEmission": { "target": "log", "targetConfig": { "logFileName": metric_log.to_string_lossy() } },
            "component": { "global": {} }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        &messaging_path,
        format!(
            r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "it-hb-{}" }} }} }}"#,
            Uuid::new_v4()
        ),
    )
    .unwrap();

    info!("building the runtime (starts the heartbeat)");
    let gg = EdgeCommonsBuilder::new("com.example.HbIt")
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

    // Wait for at least two keepalives (~1s apart).
    for _ in 0..80 {
        if count.load(Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        count.load(Ordering::SeqCst) >= 2,
        "expected >=2 state keepalives"
    );

    {
        let messages = received.lock().unwrap();
        let msg = &messages[0];
        assert_eq!(msg.header.name, "state");
        assert_eq!(msg.header.version, "1.0");
        assert_eq!(msg.body["status"], "RUNNING");
        assert!(
            msg.body["uptimeSecs"].is_u64(),
            "RUNNING carries uptimeSecs"
        );
        let identity = msg
            .identity
            .as_ref()
            .expect("state envelope carries identity");
        assert_eq!(identity.device(), thing);
        assert_eq!(identity.component(), "HbIt");
        assert_eq!(identity.instance(), "main");
    }

    // Dropping the runtime publishes the best-effort STOPPED state.
    info!("dropping the runtime (expect a STOPPED state)");
    drop(gg);
    let mut saw_stopped = false;
    for _ in 0..60 {
        if received
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.body["status"] == "STOPPED")
        {
            saw_stopped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        saw_stopped,
        "a STOPPED state should be published on graceful shutdown"
    );
    let stopped: Vec<_> = received
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.body["status"] == "STOPPED")
        .cloned()
        .collect();
    assert_eq!(stopped.len(), 1, "STOPPED is published at most once");
    assert!(
        stopped[0].body.get("uptimeSecs").is_none(),
        "STOPPED omits uptimeSecs"
    );

    let _ = std::fs::remove_dir_all(&dir);
    info!("=== PASS heartbeat_publishes_uns_state_keepalive ===");
}
