#![cfg(feature = "kafka")]
//! Integration test: `KafkaSink` against a local Kafka broker (Redpanda/Kafka on :9092).
//! **Ignored by default** (needs a broker).
//!
//! ```sh
//! docker run -d -p 9092:9092 redpandadata/redpanda redpanda start --smp 1 \
//!   --kafka-addr PLAINTEXT://0.0.0.0:9092 --advertise-kafka-addr PLAINTEXT://127.0.0.1:9092
//! cargo test -p ggstreamlog --features kafka --test kafka_redpanda -- --ignored --nocapture
//! ```
//! Override the broker with `GGSTREAMLOG_KAFKA_BOOTSTRAP` (default `127.0.0.1:9092`).
//!
//! Exercises the real path: append -> ExportEngine -> KafkaSink -> broker, then consumes the topic
//! back and asserts no records were lost.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};

use ggstreamlog::config::{BatchConfig, BufferConfig, DeliveryConfig, FsyncPolicy, OnFull};
use ggstreamlog::{EmbeddedLog, ExportEngine, KafkaSink, Record};

const N: usize = 500;

fn bootstrap() -> String {
    std::env::var("GGSTREAMLOG_KAFKA_BOOTSTRAP").unwrap_or_else(|_| "127.0.0.1:9092".to_string())
}

#[test]
#[ignore = "requires a local Kafka broker (Redpanda/Kafka) on :9092"]
fn kafka_sink_delivers_to_broker() {
    let topic = format!("ggstreamlog-it-{}", std::process::id());
    let dir = tempfile::tempdir().unwrap();
    let log = Arc::new(
        EmbeddedLog::open(BufferConfig {
            path: dir.path().to_string_lossy().into_owned(),
            segment_bytes: 1 << 20,
            max_disk_bytes: 1 << 30,
            on_full: OnFull::Block,
            fsync: FsyncPolicy::PerBatch,
            ..Default::default()
        })
        .unwrap(),
    );
    for i in 0..N {
        log.append(&Record::new("pk", 1000 + i as u64, payload(i).as_bytes())).unwrap();
    }

    let sink = KafkaSink::new(&bootstrap(), &topic, &Default::default()).expect("build KafkaSink");
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig { max_records: 100, ..Default::default() },
        DeliveryConfig { max_retries: 10, backoff_base_ms: 50, backoff_max_ms: 1000, poll_interval_ms: 20 },
    );

    let drained = wait_until(Duration::from_secs(30), || log.acked() == N as u64);
    let stats = engine.stats();
    assert!(
        drained,
        "engine did not drain: acked={} exported={} failed={} last_error={:?}",
        log.acked(),
        stats.exported_total,
        stats.failed_total,
        stats.last_error
    );
    assert_eq!(stats.exported_total, N as u64);
    assert_eq!(stats.failed_total, 0);
    engine.stop();

    // Consume the topic back and confirm no payload was lost.
    let got = consume_all(&topic);
    assert!(got >= N, "expected >= {N} records on the topic, got {got}");
    println!("OK: produced {N} records to topic {topic}; consumed {got} back");
}

fn payload(i: usize) -> String {
    format!("rec-{i:06}")
}

fn consume_all(topic: &str) -> usize {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap())
        .set("group.id", format!("ggsl-it-{}", std::process::id()))
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.commit", "false")
        .create()
        .expect("consumer");
    consumer.subscribe(&[topic]).expect("subscribe");

    let mut count = 0usize;
    let start = Instant::now();
    let mut idle = 0;
    // Poll until we've seen N or several consecutive empty polls after the tip.
    while start.elapsed() < Duration::from_secs(20) {
        match consumer.poll(Duration::from_millis(500)) {
            Some(Ok(_msg)) => {
                count += 1;
                idle = 0;
                if count >= N {
                    break;
                }
            }
            Some(Err(_)) | None => {
                idle += 1;
                if count > 0 && idle >= 4 {
                    break;
                }
            }
        }
    }
    count
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}
