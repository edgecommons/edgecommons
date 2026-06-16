//! Integration test for the top-level `GgCommons` runtime in STANDALONE mode.
//!
//! Exercises the full `GgCommonsBuilder::build` path (CLI parse, STANDALONE
//! messaging connect, FILE config load/validate, logging, metrics, heartbeat,
//! config-watch task) and every public accessor, against the local MQTT broker.
//!
//! Gated: no-op unless `GGCOMMONS_IT_MQTT=1` is set (a broker must be reachable).

use std::sync::Arc;

use ggcommons::config::Config;
use ggcommons::prelude::*;

fn skipped() -> bool {
    if std::env::var("GGCOMMONS_IT_MQTT").is_ok() {
        return false;
    }
    eprintln!("skipping STANDALONE runtime test (set GGCOMMONS_IT_MQTT=1 to enable)");
    true
}

/// A config-change listener that does nothing (for add/remove coverage).
struct NoopListener;

#[async_trait::async_trait]
impl ConfigurationChangeListener for NoopListener {
    async fn on_configuration_change(&self, _config: Arc<Config>) -> bool {
        true
    }
}

#[tokio::test]
async fn standalone_runtime_exposes_all_services_and_accessors() {
    if skipped() {
        return;
    }

    let host = std::env::var("GGCOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("GGCOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());

    let dir = std::env::temp_dir().join(format!("ggcommons-lib-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let messaging_path = dir.join("messaging.json");
    let metric_log = dir.join("metric.log");

    std::fs::write(
        &config_path,
        serde_json::json!({
            "logging": { "level": "DEBUG" },
            "metricEmission": { "target": "log", "targetConfig": { "logFileName": metric_log.to_string_lossy() } },
            "heartbeat": { "intervalSecs": 1, "measures": { "cpu": true }, "targets": [ { "type": "metric" } ] },
            "component": { "global": { "publish_interval": 2 } }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        &messaging_path,
        format!(
            r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "lib-it-{}" }} }} }}"#,
            uuid::Uuid::new_v4()
        ),
    )
    .unwrap();

    let gg = GgCommonsBuilder::new("com.example.LibIt")
        .args([
            "prog".to_string(),
            "-m".to_string(),
            "STANDALONE".to_string(),
            messaging_path.to_string_lossy().into_owned(),
            "-c".to_string(),
            "FILE".to_string(),
            config_path.to_string_lossy().into_owned(),
            "-t".to_string(),
            "lib-thing".to_string(),
        ])
        .build()
        .await
        .expect("build STANDALONE runtime");

    // Identity + args accessors.
    assert_eq!(gg.component_name(), "com.example.LibIt");
    assert!(matches!(gg.args().mode, RuntimeMode::Standalone { .. }));

    // Config snapshot accessor.
    let cfg = gg.config();
    assert_eq!(cfg.thing_name, "lib-thing");
    assert_eq!(cfg.global()["publish_interval"], 2);

    // Messaging available in STANDALONE mode; metrics always available.
    let messaging = gg.messaging().expect("messaging available in STANDALONE");
    let _metrics = gg.metrics();

    // Exercise a real publish through the wired service.
    let msg = ggcommons::messaging::message::MessageBuilder::new("Ping", "1.0")
        .from_config(&cfg)
        .payload(serde_json::json!({ "ok": true }))
        .build();
    messaging.publish("lib-it/ping", &msg).await.expect("publish");

    // Listener add/remove (identity-based remove).
    let listener: Arc<dyn ConfigurationChangeListener> = Arc::new(NoopListener);
    gg.add_config_change_listener(listener.clone());
    gg.remove_config_change_listener(&listener);

    // Dropping the runtime stops the heartbeat + watch tasks (RAII) — must not hang.
    drop(gg);
    let _ = std::fs::remove_dir_all(&dir);
}
