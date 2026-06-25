//! Integration test for FILE config hot reload through the full runtime.
//!
//! Builds GgCommons against a FILE config source, registers a config-change
//! listener, modifies the file, and asserts the snapshot updates and the listener
//! fires.
//!
//! Phase 0 note: the two-axis runtime model has no "no transport" option — a built
//! runtime always selects a concrete transport (DESIGN-core §4). On a non-Greengrass
//! build the only transport is MQTT, which needs a broker. (The old brokerless path —
//! GREENGRASS mode without the `greengrass` feature yielding `messaging = None` — was
//! the silent `Ok(None)` no-op that the §12 #4 fail-fast decision deliberately
//! removes.) These tests therefore connect HOST/MQTT and are gated: no-op unless
//! `GGCOMMONS_IT_MQTT=1` is set (a broker must be reachable). The config-reload
//! pipeline itself is also covered, broker-free, by the source-level unit tests.
//!
//! Skipped under the `greengrass` feature: there, IPC transport performs a real
//! `connect()` to the nucleus, which is unavailable in a unit-test environment.
#![cfg(not(feature = "greengrass"))]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ggcommons::config::Config;
use ggcommons::prelude::*;

fn skipped() -> bool {
    if std::env::var("GGCOMMONS_IT_MQTT").is_ok() {
        return false;
    }
    eprintln!("skipping config-hot-reload runtime test (set GGCOMMONS_IT_MQTT=1 to enable)");
    true
}

/// Write a messaging-config file for the local broker and return its path.
fn write_messaging_config(dir: &std::path::Path) -> std::path::PathBuf {
    let host = std::env::var("GGCOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("GGCOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let path = dir.join("messaging.json");
    std::fs::write(
        &path,
        format!(
            r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "reload-it-{}" }} }} }}"#,
            uuid::Uuid::new_v4()
        ),
    )
    .unwrap();
    path
}

/// Build a HOST/MQTT runtime with a FILE config source (broker required).
async fn build_runtime(component: &str, dir: &std::path::Path, config_path: &std::path::Path) -> GgCommons {
    let messaging_path = write_messaging_config(dir);
    GgCommonsBuilder::new(component.to_string())
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
            "thing-1".to_string(),
        ])
        .build()
        .await
        .expect("build")
}

/// A listener that records the latest `component.global.v` and how many times it fired.
struct RecordingListener {
    last_v: Mutex<Option<i64>>,
    count: AtomicUsize,
}

#[async_trait::async_trait]
impl ConfigurationChangeListener for RecordingListener {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        let v = config.global().get("v").and_then(serde_json::Value::as_i64);
        *self.last_v.lock().unwrap() = v;
        self.count.fetch_add(1, Ordering::SeqCst);
        true
    }
}

fn write_config(path: &std::path::Path, log_path: &std::path::Path, v: i64) {
    let contents = serde_json::json!({
        "metricEmission": { "target": "log", "targetConfig": { "logFileName": log_path.to_string_lossy() } },
        "component": { "global": { "v": v } }
    });
    std::fs::write(path, serde_json::to_vec_pretty(&contents).unwrap()).unwrap();
}

#[tokio::test]
async fn file_config_hot_reloads_and_notifies_listeners() {
    if skipped() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("ggcommons-reload-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let log_path = dir.join("metric.log");
    write_config(&config_path, &log_path, 1);

    let gg = build_runtime("com.example.ReloadTest", &dir, &config_path).await;

    assert_eq!(gg.config().global()["v"], 1);

    let listener = Arc::new(RecordingListener {
        last_v: Mutex::new(None),
        count: AtomicUsize::new(0),
    });
    gg.add_config_change_listener(listener.clone());

    // Modify the file -> the watcher should pick it up and reload.
    write_config(&config_path, &log_path, 2);

    let mut reloaded = false;
    for _ in 0..100 {
        if listener.count.load(Ordering::SeqCst) >= 1 && gg.config().global()["v"] == 2 {
            reloaded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(reloaded, "config should have hot-reloaded within the timeout");
    assert_eq!(gg.config().global()["v"], 2, "snapshot updated");
    assert_eq!(*listener.last_v.lock().unwrap(), Some(2), "listener saw new value");
    assert!(listener.count.load(Ordering::SeqCst) >= 1, "listener fired");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn multi_instance_config_is_exposed_through_the_runtime() {
    if skipped() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("ggcommons-multi-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let log_path = dir.join("metric.log");
    let contents = serde_json::json!({
        "metricEmission": { "target": "log", "targetConfig": { "logFileName": log_path.to_string_lossy() } },
        "component": {
            "global": { "publish_interval": 3 },
            "instances": [
                { "id": "lineA", "sensor": "/dev/ttyUSB0" },
                { "id": "lineB", "sensor": "/dev/ttyUSB1" }
            ]
        }
    });
    std::fs::write(&config_path, serde_json::to_vec_pretty(&contents).unwrap()).unwrap();

    let gg = build_runtime("com.example.MultiInstance", &dir, &config_path).await;

    let cfg = gg.config();
    assert_eq!(cfg.instance_ids(), vec!["lineA", "lineB"]);
    assert_eq!(
        cfg.instance("lineB").and_then(|i| i.get("sensor")).and_then(|v| v.as_str()),
        Some("/dev/ttyUSB1"),
        "per-instance config is accessible by id"
    );
    assert!(cfg.instance("missing").is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn metric_target_reconfigures_on_reload() {
    if skipped() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("ggcommons-mreload-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let log_a = dir.join("a.log");
    let log_b = dir.join("b.log");
    write_config(&config_path, &log_a, 1);

    let gg = build_runtime("com.example.MetricReload", &dir, &config_path).await;

    let metrics = gg.metrics();
    metrics.define_metric(MetricBuilder::create("m").add_measure("count", "Count", 60).build());
    let mut values = HashMap::new();
    values.insert("count".to_string(), 1.0);
    metrics.emit_metric_now("m", values.clone()).await.unwrap();
    metrics.flush_metrics().await.unwrap();
    assert!(
        !std::fs::read_to_string(&log_a).unwrap().trim().is_empty(),
        "first metric goes to the original log file"
    );

    // Register a listener that fires AFTER the internal metric-target listener (it is
    // registered earlier, so by the time this fires the target has been rebuilt).
    let listener = Arc::new(RecordingListener {
        last_v: Mutex::new(None),
        count: AtomicUsize::new(0),
    });
    gg.add_config_change_listener(listener.clone());

    // Hot-reload: point the metric log at a different file.
    write_config(&config_path, &log_b, 2);
    for _ in 0..100 {
        if listener.count.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(listener.count.load(Ordering::SeqCst) >= 1, "reload happened");

    let mut values2 = HashMap::new();
    values2.insert("count".to_string(), 2.0);
    metrics.emit_metric_now("m", values2).await.unwrap();
    metrics.flush_metrics().await.unwrap();
    assert!(
        !std::fs::read_to_string(&log_b).unwrap_or_default().trim().is_empty(),
        "after reload, metrics go to the reconfigured log file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
