#![cfg(feature = "kafka")]
//! Integration test: `KafkaSink` against a local Kafka broker (Redpanda/Kafka on :9092).
//! **Ignored by default** (needs a broker).
//!
//! ```sh
//! docker run -d -p 9092:9092 redpandadata/redpanda redpanda start --smp 1 \
//!   --kafka-addr PLAINTEXT://0.0.0.0:9092 --advertise-kafka-addr PLAINTEXT://127.0.0.1:9092
//! cargo test -p edgestreamlog --features kafka --test kafka_redpanda -- --ignored --nocapture
//! ```
//! Override the broker with `EDGESTREAMLOG_KAFKA_BOOTSTRAP` (default `127.0.0.1:9092`).
//!
//! Exercises the real path: append -> ExportEngine -> KafkaSink -> broker, then consumes the topic
//! back and asserts no records were lost.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};

use edgestreamlog::config::{BatchConfig, BufferConfig, DeliveryConfig, FsyncPolicy, OnFull};
use edgestreamlog::{EmbeddedLog, ExportEngine, KafkaSink, Record, SinkPayloadFormat};

const N: usize = 500;

fn bootstrap() -> String {
    std::env::var("EDGESTREAMLOG_KAFKA_BOOTSTRAP").unwrap_or_else(|_| "127.0.0.1:9092".to_string())
}

#[test]
#[ignore = "requires a local Kafka broker (Redpanda/Kafka) on :9092"]
fn kafka_sink_delivers_to_broker() {
    let topic = format!("edgestreamlog-it-{}", std::process::id());
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
        log.append(&Record::new("pk", 1000 + i as u64, payload(i)))
            .unwrap();
    }

    let sink = KafkaSink::new_with_payload_format(
        &bootstrap(),
        &topic,
        &Default::default(),
        SinkPayloadFormat::Protobuf,
    )
    .expect("build KafkaSink");
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        BatchConfig {
            max_records: 100,
            ..Default::default()
        },
        DeliveryConfig {
            max_retries: 10,
            backoff_base_ms: 50,
            backoff_max_ms: 1000,
            poll_interval_ms: 20,
        },
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

fn payload(i: usize) -> Vec<u8> {
    edgecommons_opaque_payload(&format!("rec-{i:06}"))
}

fn edgecommons_opaque_payload(body: &str) -> Vec<u8> {
    let mut header = Vec::new();
    put_string(&mut header, 1, "FramePreview");
    put_string(&mut header, 2, "1.0");
    put_varint_field(&mut header, 3, 1_704_067_200_123);
    put_string(&mut header, 4, "kafka-redpanda-it");

    let mut msg = Vec::new();
    put_bytes(&mut msg, 1, &header);
    put_string(&mut msg, 4, "text/plain");
    put_bytes(&mut msg, 31, body.as_bytes());
    msg
}

fn put_string(out: &mut Vec<u8>, field: u64, value: &str) {
    put_bytes(out, field, value.as_bytes());
}

fn put_bytes(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    put_varint(out, field << 3);
    put_varint(out, value);
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn consume_all(topic: &str) -> usize {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap())
        .set("group.id", format!("esl-it-{}", std::process::id()))
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
