#![cfg(feature = "kinesis")]
//! Integration test: `KinesisSink` against a local AWS emulator (floci or LocalStack — both
//! speak the Kinesis API on `:4566`). **Ignored by default** (needs the emulator running).
//!
//! ```sh
//! docker run -d -p 4566:4566 floci/floci:latest          # or localstack/localstack
//! cargo test -p ggstreamlog --features kinesis --test kinesis_floci -- --ignored --nocapture
//! ```
//! Override the endpoint with `GGSTREAMLOG_KINESIS_ENDPOINT` (default `http://localhost:4566`).
//!
//! Exercises the real path: AWS credential chain → `PutRecords` → per-batch ack → log commit,
//! then reads every shard back and asserts no records were lost.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_sdk_kinesis::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_kinesis::types::ShardIteratorType;
use aws_sdk_kinesis::Client;

use ggstreamlog::config::{BatchConfig, BufferConfig, DeliveryConfig, FsyncPolicy, OnFull};
use ggstreamlog::{EmbeddedLog, ExportEngine, KinesisSink, Record};

const REGION: &str = "us-east-1";
const N: usize = 200;
const SHARDS: i32 = 2;

fn endpoint() -> String {
    std::env::var("GGSTREAMLOG_KINESIS_ENDPOINT").unwrap_or_else(|_| "http://localhost:4566".into())
}

fn admin(rt: &tokio::runtime::Runtime) -> Client {
    rt.block_on(async {
        let conf = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(REGION))
            .endpoint_url(endpoint())
            .credentials_provider(Credentials::new("test", "test", None, None, "floci-it"))
            .load()
            .await;
        Client::new(&conf)
    })
}

#[test]
#[ignore = "requires a local AWS emulator (floci/LocalStack) on :4566"]
fn kinesis_sink_delivers_to_emulator() {
    // KinesisSink uses the default credential chain; feed it static creds + region via env so it
    // talks to the emulator exactly as it would to real AWS.
    std::env::set_var("AWS_ACCESS_KEY_ID", "test");
    std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
    std::env::set_var("AWS_DEFAULT_REGION", REGION);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = admin(&rt);
    let stream = format!("ggstreamlog-it-{}", std::process::id());

    // Fresh stream.
    rt.block_on(async {
        let _ = client.delete_stream().stream_name(&stream).send().await; // ignore "not found"
        client
            .create_stream()
            .stream_name(&stream)
            .shard_count(SHARDS)
            .send()
            .await
            .expect("create_stream");
    });
    wait_active(&rt, &client, &stream);

    // Buffer N records, then drain via the real sink.
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
        // Spread across partition keys so both shards receive data.
        log.append(&Record::new(format!("pk-{}", i % 4), 1000 + i as u64, payload(i).as_bytes()))
            .unwrap();
    }

    let sink = KinesisSink::new(stream.clone(), Some(REGION.to_string()), Some(endpoint()))
        .expect("build KinesisSink");
    let engine = ExportEngine::start(
        Arc::clone(&log),
        Box::new(sink),
        // PutRecords caps: stay well under 500 records / 5 MiB.
        BatchConfig { max_records: 100, max_bytes: 1 << 20, ..Default::default() },
        DeliveryConfig { max_retries: 8, backoff_base_ms: 20, backoff_max_ms: 500, poll_interval_ms: 20 },
    );

    // Wait until the engine has committed everything (acked cursor reached N).
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

    // Read every shard back and confirm no payload was lost.
    let got = read_all_payloads(&rt, &client, &stream);
    let expected: HashSet<String> = (0..N).map(payload).collect();
    let missing: Vec<&String> = expected.iter().filter(|p| !got.contains(*p)).collect();
    assert!(missing.is_empty(), "{} payloads missing from Kinesis, e.g. {:?}", missing.len(), missing.first());
    assert!(got.len() >= N, "expected >= {N} records in Kinesis, got {}", got.len());

    // Cleanup.
    rt.block_on(async {
        let _ = client.delete_stream().stream_name(&stream).send().await;
    });
    println!("OK: delivered {N} records across {SHARDS} shards; read back {} unique payloads", got.len());
}

fn payload(i: usize) -> String {
    format!("rec-{i:06}")
}

fn wait_active(rt: &tokio::runtime::Runtime, client: &Client, stream: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        let active = rt.block_on(async {
            client
                .describe_stream_summary()
                .stream_name(stream)
                .send()
                .await
                .ok()
                .and_then(|o| o.stream_description_summary().map(|s| s.stream_status().clone()))
        });
        if matches!(active, Some(aws_sdk_kinesis::types::StreamStatus::Active)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    panic!("stream {stream} did not become ACTIVE within 30s");
}

/// Read all records from every shard (TRIM_HORIZON), returning the set of payload strings.
fn read_all_payloads(rt: &tokio::runtime::Runtime, client: &Client, stream: &str) -> HashSet<String> {
    rt.block_on(async {
        let mut out = HashSet::new();
        let shards = client
            .list_shards()
            .stream_name(stream)
            .send()
            .await
            .expect("list_shards")
            .shards()
            .to_vec();
        for shard in shards {
            let mut iter = client
                .get_shard_iterator()
                .stream_name(stream)
                .shard_id(shard.shard_id())
                .shard_iterator_type(ShardIteratorType::TrimHorizon)
                .send()
                .await
                .expect("get_shard_iterator")
                .shard_iterator()
                .map(str::to_string);

            let mut empty_polls = 0;
            while let Some(it) = iter.clone() {
                let resp = client.get_records().shard_iterator(it).limit(1000).send().await
                    .expect("get_records");
                let recs = resp.records();
                if recs.is_empty() {
                    empty_polls += 1;
                    // Stop once caught up to the tip and a couple of polls return nothing.
                    if resp.millis_behind_latest() == Some(0) && empty_polls >= 2 {
                        break;
                    }
                } else {
                    empty_polls = 0;
                    for r in recs {
                        out.insert(String::from_utf8_lossy(r.data().as_ref()).into_owned());
                    }
                }
                iter = resp.next_shard_iterator().map(str::to_string);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        out
    })
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
