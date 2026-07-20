//! Integration test for the top-level [`edgecommons::EdgeCommonsBuilder::build`] runtime that runs
//! **entirely in-process** — no external broker required, so it executes on a stock CI machine.
//!
//! A tiny embedded MQTT server ([`spawn_fake_broker`]) accepts the provider's TCP connection and
//! replies to CONNECT with CONNACK (and to SUBSCRIBE/PINGREQ correctly), which is all the standalone
//! dual-MQTT provider needs to confirm its connection. That lets us drive the *whole* builder path —
//! CLI parse, MQTT connect, FILE config load + validate, logging init, metrics, heartbeat, the
//! credentials / streaming / parameters wiring, the health/SIGTERM plumbing, and every public
//! accessor — and then assert real, observable behavior (connected state, resolved config, template
//! resolution, working secret/stream/parameter services).
//!
//! This complements `lib_standalone.rs` (which targets a real broker behind `EDGECOMMONS_IT_MQTT`).
//!
//! Feature-gated: it exercises `gg.streams()`/`gg.credentials()`/`gg.parameters()`, so it only compiles
//! when those features are enabled (mirrors `tests/credentials_local.rs`). `standalone` is a default
//! feature, so the MQTT path needs no gate. Under default features (just `standalone`) this file is
//! cfg'd out, keeping `cargo clippy --all-targets` / `cargo test` green.
#![cfg(all(feature = "streaming", feature = "credentials", feature = "parameters"))]

use std::sync::Arc;

use edgecommons::config::Config;
use edgecommons::prelude::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A config-change listener that records nothing (used only for add/remove identity coverage).
struct NoopListener;

#[async_trait::async_trait]
impl ConfigurationChangeListener for NoopListener {
    async fn on_configuration_change(&self, _config: Arc<Config>) -> bool {
        true
    }
}

/// Spawn a minimal in-process MQTT v3.1.1 server on an ephemeral loopback port and return the port.
///
/// It frames packets correctly (fixed header + remaining-length varint + payload) and answers the
/// handful the provider can send: CONNECT→CONNACK(accepted), SUBSCRIBE→SUBACK(granted QoS0),
/// PINGREQ→PINGRESP, DISCONNECT→close. PUBLISH (QoS0) and anything else are drained. This is enough
/// for `MqttProvider::connect` to observe its first CONNACK and mark the connection live.
async fn spawn_fake_broker() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((sock, _)) => {
                    tokio::spawn(handle_conn(sock));
                }
                Err(_) => return,
            }
        }
    });
    port
}

async fn handle_conn(mut sock: TcpStream) {
    loop {
        // Fixed header.
        let mut hdr = [0u8; 1];
        if sock.read_exact(&mut hdr).await.is_err() {
            return;
        }
        let ptype = hdr[0] >> 4;

        // Remaining-length varint (1..=4 bytes).
        let mut rem: usize = 0;
        let mut mult: usize = 1;
        loop {
            let mut b = [0u8; 1];
            if sock.read_exact(&mut b).await.is_err() {
                return;
            }
            rem += (b[0] & 0x7f) as usize * mult;
            if b[0] & 0x80 == 0 {
                break;
            }
            mult *= 128;
        }

        let mut payload = vec![0u8; rem];
        if rem > 0 && sock.read_exact(&mut payload).await.is_err() {
            return;
        }

        match ptype {
            1 => {
                // CONNECT -> CONNACK (session present = 0, return code 0 = accepted).
                if sock.write_all(&[0x20, 0x02, 0x00, 0x00]).await.is_err() {
                    return;
                }
            }
            8 => {
                // SUBSCRIBE -> SUBACK echoing the packet id, granting QoS0.
                if payload.len() >= 2
                    && sock
                        .write_all(&[0x90, 0x03, payload[0], payload[1], 0x00])
                        .await
                        .is_err()
                {
                    return;
                }
            }
            12 => {
                // PINGREQ -> PINGRESP.
                if sock.write_all(&[0xD0, 0x00]).await.is_err() {
                    return;
                }
            }
            14 => return, // DISCONNECT
            _ => {}       // PUBLISH (QoS0) and others: nothing to ack.
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builds_full_runtime_against_inprocess_broker() {
    let port = spawn_fake_broker().await;

    let dir = std::env::temp_dir().join(format!("edgecommons-inproc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.json");
    let messaging_path = dir.join("messaging.json");
    let metric_log = dir.join("metric.log");

    std::fs::write(
        &config_path,
        serde_json::json!({
            "logging": { "level": "DEBUG" },
            "metricEmission": { "target": "log",
                "targetConfig": { "logFileName": metric_log.to_string_lossy() } },
            "heartbeat": { "intervalSecs": 1, "measures": { "cpu": true } },
            "streaming": { "streams": [ {
                "name": "telemetry",
                "sink": { "type": "kinesis", "streamName": "ts-{ThingName}" },
                "buffer": { "path": dir.join("stream-{ThingName}").to_string_lossy(),
                            "segmentBytes": 65536, "maxDiskBytes": 1048576, "onFull": "block" }
            } ] },
            "credentials": { "vault": {
                "path": dir.join("vault-{ThingName}").to_string_lossy(),
                "keyProvider": { "type": "file" }
            } },
            "parameters": { "source": { "type": "env", "prefix": "GGINPROC_" } },
            "component": { "global": { "publish_interval": 7 } }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        &messaging_path,
        format!(
            r#"{{ "messaging": {{ "local": {{ "host": "127.0.0.1", "port": {port}, "clientId": "inproc-{}" }} }} }}"#,
            uuid::Uuid::new_v4()
        ),
    )
    .unwrap();

    let gg = EdgeCommonsBuilder::new("com.example.InProc")
        // false is a documented no-op on the MQTT transport; here it just exercises the setter.
        .receive_own_messages(false)
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
            "inproc-thing".to_string(),
        ])
        .build()
        .await
        .expect("build the full runtime against the in-process broker");

    // Identity + args accessors.
    assert_eq!(gg.component_name(), "com.example.InProc");
    assert_eq!(gg.args().platform, Platform::Host);
    assert_eq!(gg.args().transport, Transport::Mqtt);

    // Config snapshot accessor + template-substituted component config.
    let cfg = gg.config();
    assert_eq!(cfg.thing_name, "inproc-thing");
    assert_eq!(cfg.global()["publish_interval"], 7);

    // Messaging is wired and the CONNACK from the fake broker has been observed.
    let messaging = gg.messaging().expect("messaging available in STANDALONE");
    assert!(
        messaging.connected(),
        "the fake broker's CONNACK should mark the link live"
    );
    let _metrics = gg.metrics();

    // A real publish flows through the wired provider to the in-process broker (QoS0, no ack).
    let msg = edgecommons::messaging::message::MessageBuilder::new("Ping", "1.0")
        .from_config(&cfg)
        .payload(serde_json::json!({ "ok": true }))
        .build();
    messaging
        .publish("inproc/ping", &msg)
        .await
        .expect("publish");

    // The raw device-bus provider affordance (relay/bridge guard bypass): with a
    // transport wired, the runtime hands back its OWN live provider — the same
    // connection `messaging()` reported connected — and a relay can publish raw bytes
    // through it BELOW the reserved-class guard that the normal service enforces.
    let raw = gg
        .raw_device_provider()
        .expect("raw device provider is available when a transport is wired");
    assert!(
        raw.connected(),
        "the raw provider shares the runtime's live device-bus connection"
    );
    // The guarded MessagingService path refuses a reserved UNS class...
    assert!(
        matches!(
            messaging.publish("ecv1/d/c/i/state", &msg).await,
            Err(edgecommons::EdgeCommonsError::ReservedTopic(_))
        ),
        "the guarded service path still refuses reserved classes"
    );
    // ...but the raw provider forwards raw bytes to that same reserved class, below
    // the guard (QoS0 to the in-process broker: enqueued, drained, no ack).
    raw.publish(
        "ecv1/d/c/i/state",
        b"raw-relay-bytes".to_vec(),
        edgecommons::messaging::Destination::Local,
        edgecommons::messaging::Qos::AtMostOnce,
    )
    .await
    .expect("raw provider publishes below the reserved-class guard");

    // Streaming wired from config with the {ThingName} buffer-path template resolved.
    let streams = gg.streams();
    assert_eq!(streams.stream_names(), vec!["telemetry"]);
    let h = streams.stream("telemetry").expect("configured stream");
    for i in 0..3u64 {
        h.append(edgecommons::streaming::StreamRecord::new(
            "k",
            1000 + i,
            b"v",
        ))
        .unwrap();
    }
    h.flush().unwrap();
    assert_eq!(streams.stats("telemetry").expect("stats").appended_total, 3);
    assert!(
        dir.join("stream-inproc-thing").is_dir(),
        "buffer path template resolved"
    );

    // Credentials wired from config with the {ThingName} vault-path template resolved.
    let creds = gg.credentials().expect("credentials configured");
    creds
        .put(
            "db/password",
            b"s3cr3t",
            edgecommons::credentials::PutOptions::default(),
        )
        .unwrap();
    assert_eq!(creds.get_string("db/password").unwrap().unwrap(), "s3cr3t");
    assert!(
        dir.join("vault-inproc-thing").exists(),
        "vault path template resolved"
    );

    // Parameters wired from config (env source); the service is present.
    assert!(
        gg.parameters().is_some(),
        "parameters service should be wired from the config section"
    );

    // Readiness + shutdown flags.
    gg.set_ready(false);
    gg.set_ready(true);
    assert!(!gg.is_shutting_down());

    // Listener add/remove (identity-based remove).
    let listener: Arc<dyn ConfigurationChangeListener> = Arc::new(NoopListener);
    gg.add_config_change_listener(listener.clone());
    gg.remove_config_change_listener(&listener);

    // Dropping the runtime stops the heartbeat + watch tasks (RAII) — must not hang.
    drop(gg);
    let _ = std::fs::remove_dir_all(&dir);
}
