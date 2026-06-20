//! `loadgen` — the telemetry-streaming load generator (perf harness, spec §15.5).
//!
//! Drives the `ggstreamlog` ingest + drain paths end-to-end and emits **one JSON results record**
//! per run (→ the baseline matrix + regression comparator, §15.8). There is **no fixed throughput
//! target**: the goal is per-(target × config) baselines, the throughput↔durability curve, and the
//! bottleneck (CPU vs disk vs fsync vs network).
//!
//! ```text
//! cargo run --release --example loadgen -- \
//!   --scenario S1 --payload 1024 --threads 4 --duration 10 \
//!   --fsync PerBatch --segment-bytes 67108864 --on-full dropOldest --sink none \
//!   --path /mnt/data/ggsl-bench --target-name lab-5950x --git-sha $(git rev-parse --short HEAD)
//! ```
//!
//! Flags: `--rate <r|unbounded>` `--payload <bytes>` `--threads <n>` `--duration <s>`
//! `--count <n>` `--fsync <PerBatch|Interval|Always>` `--segment-bytes <n>` `--max-disk-bytes <n>`
//! `--on-full <dropOldest|block|rejectNew>` `--sink <none|fake-rate:<r>|disconnect:<secs>|kinesis:<stream>|localstack:<stream>>`
//! `--scenario <S1..S8>` `--path <dir>` `--batch-records <n>` `--target-name <s>` `--git-sha <s>`
//! `--results-dir <dir>` `--out <file>` `--drop-caches`.
//!
//! Sinks (offline/free unless `kinesis`): `none` = no export engine, so the **ingest/write path is
//! measured in isolation** (S1/S2); `fake-rate:<r>` caps drain to model a slow sink (the drain
//! isolator); `disconnect:<secs>` fails for N s then acks (backlog→drain); `kinesis:`/`localstack:`
//! real `PutRecords` (needs the `kinesis` cargo feature).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use ggstreamlog::config::{BatchConfig, BufferConfig, DeliveryConfig, FsyncPolicy, OnFull};
use ggstreamlog::export::{ExportRecord, SendOutcome, Sink};
use ggstreamlog::{EmbeddedLog, ExportEngine, Record};

// ============================ argument parsing ============================

struct Args {
    raw: HashMap<String, String>,
}

impl Args {
    fn parse() -> Self {
        let mut raw = HashMap::new();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            if let Some(key) = a.strip_prefix("--") {
                // Boolean flags (no value) vs key/value.
                if key == "drop-caches" {
                    raw.insert(key.to_string(), "true".to_string());
                } else if let Some(v) = it.next() {
                    raw.insert(key.to_string(), v);
                }
            }
        }
        Self { raw }
    }
    fn str(&self, k: &str, default: &str) -> String {
        self.raw.get(k).cloned().unwrap_or_else(|| default.to_string())
    }
    fn opt(&self, k: &str) -> Option<String> {
        self.raw.get(k).cloned()
    }
    fn u64(&self, k: &str, default: u64) -> u64 {
        self.raw.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn usize(&self, k: &str, default: usize) -> usize {
        self.raw.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn f64(&self, k: &str, default: f64) -> f64 {
        self.raw.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn flag(&self, k: &str) -> bool {
        self.raw.contains_key(k)
    }
}

fn parse_fsync(s: &str) -> FsyncPolicy {
    match s.to_ascii_lowercase().as_str() {
        "interval" => FsyncPolicy::Interval,
        "always" => FsyncPolicy::Always,
        _ => FsyncPolicy::PerBatch,
    }
}

fn parse_on_full(s: &str) -> OnFull {
    match s.to_ascii_lowercase().as_str() {
        "block" => OnFull::Block,
        "rejectnew" => OnFull::RejectNew,
        _ => OnFull::DropOldest,
    }
}

/// Parsed `--sink` spec.
enum SinkSpec {
    None,
    FakeRate(f64),
    Disconnect(Duration),
    #[allow(dead_code)]
    Kinesis { stream: String, endpoint: Option<String> },
}

fn parse_sink(s: &str) -> SinkSpec {
    let (kind, arg) = s.split_once(':').unwrap_or((s, ""));
    match kind.to_ascii_lowercase().as_str() {
        "fake-rate" => SinkSpec::FakeRate(arg.parse().unwrap_or(10_000.0)),
        "disconnect" => SinkSpec::Disconnect(Duration::from_secs_f64(arg.parse().unwrap_or(5.0))),
        "kinesis" => SinkSpec::Kinesis { stream: arg.to_string(), endpoint: None },
        "localstack" | "floci" => SinkSpec::Kinesis {
            stream: arg.to_string(),
            endpoint: Some("http://localhost:4566".to_string()),
        },
        _ => SinkSpec::None,
    }
}

// ============================ perf sinks ============================
// Lightweight counting sinks (do NOT store payloads — perf runs push millions of records).

/// Rate-limited sink: caps drain to ~`rate` records/s to model a slow/limited downstream.
struct RateLimitedSink {
    rate: f64,
    delivered: Arc<AtomicU64>,
    start: Instant,
    sent: u64,
}
impl Sink for RateLimitedSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        self.sent += batch.len() as u64;
        // Sleep until this batch's records are "allowed" by the target rate.
        let allowed_at = self.start + Duration::from_secs_f64(self.sent as f64 / self.rate);
        let now = Instant::now();
        if allowed_at > now {
            std::thread::sleep(allowed_at - now);
        }
        self.delivered.fetch_add(batch.len() as u64, Ordering::Relaxed);
        SendOutcome::AllAcked
    }
}

/// Disconnected-then-ack sink: fails (retryable) until `until`, then acks — backlog → drain.
struct DisconnectSink {
    until: Instant,
    delivered: Arc<AtomicU64>,
}
impl Sink for DisconnectSink {
    fn send(&mut self, batch: &[ExportRecord<'_>]) -> SendOutcome {
        if Instant::now() < self.until {
            return SendOutcome::Failed { retryable: true, error: "simulated outage".into() };
        }
        self.delivered.fetch_add(batch.len() as u64, Ordering::Relaxed);
        SendOutcome::AllAcked
    }
}

fn build_sink(spec: &SinkSpec, delivered: Arc<AtomicU64>) -> Option<Box<dyn Sink>> {
    match spec {
        // `none` => no export engine at all, so the ingest path is measured in isolation (S1/S2).
        // (An instant in-process drain would otherwise contend for the buffer lock and re-read the
        // active segment each poll, polluting the pure-ingest numbers.)
        SinkSpec::None => None,
        SinkSpec::FakeRate(r) => {
            Some(Box::new(RateLimitedSink { rate: *r, delivered, start: Instant::now(), sent: 0 }))
        }
        SinkSpec::Disconnect(d) => {
            Some(Box::new(DisconnectSink { until: Instant::now() + *d, delivered }))
        }
        SinkSpec::Kinesis { stream, endpoint } => build_kinesis(stream, endpoint.clone(), delivered),
    }
}

#[cfg(feature = "kinesis")]
fn build_kinesis(stream: &str, endpoint: Option<String>, _delivered: Arc<AtomicU64>) -> Option<Box<dyn Sink>> {
    // The real sink reports its own delivery; the harness reads exported_total from EngineStats.
    let region = std::env::var("AWS_DEFAULT_REGION").ok().or(Some("us-east-1".to_string()));
    match ggstreamlog::KinesisSink::new(stream.to_string(), region, endpoint) {
        Ok(s) => Some(Box::new(s)),
        Err(e) => {
            eprintln!("loadgen: failed to build KinesisSink: {e}");
            None
        }
    }
}

#[cfg(not(feature = "kinesis"))]
fn build_kinesis(_s: &str, _e: Option<String>, _d: Arc<AtomicU64>) -> Option<Box<dyn Sink>> {
    eprintln!("loadgen: --sink kinesis requires the `kinesis` cargo feature; using buffer-only");
    None
}

// ============================ latency capture ============================

/// Bounded-memory latency sample buffer (decimates above a cap to bound RSS during long runs).
struct Latencies {
    v: Vec<u64>,
    stride: u64,
    seen: u64,
}
impl Latencies {
    fn new() -> Self {
        Self { v: Vec::new(), stride: 1, seen: 0 }
    }
    fn push(&mut self, ns: u64) {
        self.seen += 1;
        if self.seen % self.stride == 0 {
            self.v.push(ns);
            if self.v.len() >= 8_000_000 {
                // Keep every other sample; double the stride going forward.
                let mut i = 0;
                self.v.retain(|_| {
                    i += 1;
                    i % 2 == 0
                });
                self.stride *= 2;
            }
        }
    }
    fn merge(&mut self, mut other: Latencies) {
        self.v.append(&mut other.v);
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

// ============================ environment block ============================

#[derive(Serialize, Default)]
struct EnvBlock {
    os: String,
    arch: String,
    cpu_model: Option<String>,
    cpu_cores: Option<usize>,
    total_mem_bytes: Option<u64>,
    kernel: Option<String>,
    fs_mount: Option<String>,
    build_profile: String,
    page_cache_dropped: bool,
}

fn capture_env(path: &Path, drop_caches: bool) -> EnvBlock {
    EnvBlock {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_model: linux_cpu_model(),
        cpu_cores: std::thread::available_parallelism().ok().map(|n| n.get()),
        total_mem_bytes: linux_total_mem(),
        kernel: linux_kernel(),
        fs_mount: linux_fs_for_path(path),
        build_profile: if cfg!(debug_assertions) { "debug".into() } else { "release".into() },
        page_cache_dropped: if drop_caches { try_drop_caches() } else { false },
    }
}

// ---- Linux /proc helpers (None / no-op on other platforms) ----

#[cfg(target_os = "linux")]
fn rss_bytes() -> Option<u64> {
    let s = fs::read_to_string("/proc/self/status").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}
#[cfg(not(target_os = "linux"))]
fn rss_bytes() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn cpu_secs() -> Option<f64> {
    // /proc/self/stat fields 14 (utime) + 15 (stime) in clock ticks; USER_HZ assumed 100.
    let s = fs::read_to_string("/proc/self/stat").ok()?;
    let close = s.rfind(')')?; // comm may contain spaces/parens; fields start after the last ')'
    let fields: Vec<&str> = s[close + 1..].split_whitespace().collect();
    // After ')', field index 0 = state(3rd overall); utime is overall #14 → index 11, stime → 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) as f64 / 100.0)
}
#[cfg(not(target_os = "linux"))]
fn cpu_secs() -> Option<f64> {
    None
}

#[cfg(target_os = "linux")]
fn linux_cpu_model() -> Option<String> {
    let s = fs::read_to_string("/proc/cpuinfo").ok()?;
    s.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}
#[cfg(not(target_os = "linux"))]
fn linux_cpu_model() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn linux_total_mem() -> Option<u64> {
    let s = fs::read_to_string("/proc/meminfo").ok()?;
    let line = s.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}
#[cfg(not(target_os = "linux"))]
fn linux_total_mem() -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn linux_kernel() -> Option<String> {
    fs::read_to_string("/proc/sys/kernel/osrelease").ok().map(|s| s.trim().to_string())
}
#[cfg(not(target_os = "linux"))]
fn linux_kernel() -> Option<String> {
    None
}

/// Best-effort: the mount point + fs type backing `path` (Linux /proc/mounts).
#[cfg(target_os = "linux")]
fn linux_fs_for_path(path: &Path) -> Option<String> {
    let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mounts = fs::read_to_string("/proc/mounts").ok()?;
    let mut best: Option<(usize, String)> = None;
    for line in mounts.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        let (mp, fstype) = (f[1], f[2]);
        if abs.starts_with(mp) && best.as_ref().map(|(l, _)| mp.len() > *l).unwrap_or(true) {
            best = Some((mp.len(), format!("{mp} ({fstype})")));
        }
    }
    best.map(|(_, s)| s)
}
#[cfg(not(target_os = "linux"))]
fn linux_fs_for_path(_path: &Path) -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn try_drop_caches() -> bool {
    // Needs root; ignore failure (we then measure with a warm cache and say so).
    let _ = fs::write("/proc/sys/vm/drop_caches", "3\n");
    fs::write("/proc/sys/vm/drop_caches", "3\n").is_ok()
}
#[cfg(not(target_os = "linux"))]
fn try_drop_caches() -> bool {
    false
}

// ============================ results ============================

#[derive(Serialize)]
struct Results {
    schema: &'static str,
    scenario: String,
    target_name: String,
    git_sha: String,
    timestamp_unix_ms: u128,
    config: ConfigBlock,
    env: EnvBlock,
    metrics: Metrics,
}

#[derive(Serialize)]
struct ConfigBlock {
    payload_bytes: usize,
    threads: usize,
    rate: Option<f64>,
    duration_s: f64,
    count: Option<u64>,
    fsync: String,
    segment_bytes: u64,
    max_disk_bytes: u64,
    on_full: String,
    sink: String,
    batch_records: usize,
    path: String,
}

#[derive(Serialize, Default)]
struct Metrics {
    appended_total: u64,
    exported_total: u64,
    dropped_total: u64,
    rejected_total: u64,
    duration_s: f64,
    throughput_rps: f64,
    throughput_mb_s: f64,
    logical_write_mb_s: f64,
    append_p50_ns: u64,
    append_p99_ns: u64,
    append_p999_ns: u64,
    append_max_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    e2e_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    e2e_p99_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    export_lag_p99_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drain_rate_rps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_to_catchup_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backlog_peak: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    potential_duplicates: Option<u64>,
    disk_bytes_final: u64,
    rss_start_bytes: Option<u64>,
    rss_peak_bytes: Option<u64>,
    cpu_percent: Option<f64>,
}

// ============================ producer ============================

#[allow(clippy::too_many_arguments)]
fn run_producers(
    log: &Arc<EmbeddedLog>,
    threads: usize,
    payload_bytes: usize,
    rate: Option<f64>,
    per_thread_count: Option<u64>,
    deadline: Option<Instant>,
    on_full: OnFull,
) -> (u64, u64, Latencies) {
    let appended = Arc::new(AtomicU64::new(0));
    let rejected = Arc::new(AtomicU64::new(0));
    let merged = Arc::new(Mutex::new(Latencies::new()));
    let per_thread_rate = rate.map(|r| r / threads as f64);

    std::thread::scope(|scope| {
        for tid in 0..threads {
            let log = Arc::clone(log);
            let appended = Arc::clone(&appended);
            let rejected = Arc::clone(&rejected);
            let merged = Arc::clone(&merged);
            scope.spawn(move || {
                let payload = vec![b'x'; payload_bytes];
                let pk = format!("t{tid}");
                let mut lat = Latencies::new();
                let start = Instant::now();
                let mut j: u64 = 0;
                loop {
                    if let Some(c) = per_thread_count {
                        if j >= c {
                            break;
                        }
                    }
                    if let Some(dl) = deadline {
                        if Instant::now() >= dl {
                            break;
                        }
                    }
                    if let Some(ptr) = per_thread_rate {
                        let target = start + Duration::from_secs_f64(j as f64 / ptr);
                        let now = Instant::now();
                        if target > now {
                            std::thread::sleep(target - now);
                        }
                    }
                    // Real wall-clock timestamp so export-lag (now - record_ts) is meaningful.
                    let rec = Record::new(pk.clone(), now_ms(), payload.clone());
                    let t0 = Instant::now();
                    match log.append(&rec) {
                        Ok(()) => {
                            lat.push(t0.elapsed().as_nanos() as u64);
                            appended.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(ggstreamlog::GgStreamError::BufferFull) if on_full == OnFull::RejectNew => {
                            rejected.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            eprintln!("loadgen: append error: {e}");
                            break;
                        }
                    }
                    j += 1;
                }
                merged.lock().unwrap().merge(lat);
            });
        }
    });

    let lat = Arc::try_unwrap(merged).ok().unwrap().into_inner().unwrap();
    (appended.load(Ordering::Relaxed), rejected.load(Ordering::Relaxed), lat)
}

// ============================ main ============================

fn main() {
    let args = Args::parse();

    let scenario = args.str("scenario", "S1").to_ascii_uppercase();
    let payload = args.usize("payload", 1024);
    let threads = args.usize("threads", 1).max(1);
    let rate = args.opt("rate").and_then(|s| if s == "unbounded" { None } else { s.parse().ok() });
    let duration_s = args.f64("duration", 10.0);
    let count = args.opt("count").and_then(|s| s.parse().ok());
    let fsync = parse_fsync(&args.str("fsync", "PerBatch"));
    let segment_bytes = args.u64("segment-bytes", 64 * 1024 * 1024);
    let max_disk_bytes = args.u64("max-disk-bytes", 1024 * 1024 * 1024);
    let on_full = parse_on_full(&args.str("on-full", "dropOldest"));
    let sink_str = args.str("sink", "none");
    let sink_spec = parse_sink(&sink_str);
    let batch_records = args.usize("batch-records", 500);
    let drop_caches = args.flag("drop-caches");
    let path = PathBuf::from(args.str("path", "./ggsl-loadgen"));

    let buffer = BufferConfig {
        path: path.to_string_lossy().into_owned(),
        segment_bytes,
        max_disk_bytes,
        on_full,
        fsync,
        ..Default::default()
    };
    let batch = BatchConfig { max_records: batch_records, ..Default::default() };
    let delivery = DeliveryConfig { poll_interval_ms: 5, ..Default::default() };

    // Fresh run dir.
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create buffer dir");

    let env = capture_env(&path, drop_caches);
    let rss_start = rss_bytes();
    let cpu_start = cpu_secs();
    let wall_start = Instant::now();

    let mut metrics = Metrics { rss_start_bytes: rss_start, ..Default::default() };

    match scenario.as_str() {
        "S3" => run_recovery(&buffer, count.unwrap_or(1_000_000), payload, &mut metrics),
        _ => run_streaming(
            &scenario, &buffer, &batch, &delivery, threads, payload, rate, duration_s, count,
            on_full, &sink_spec, &mut metrics,
        ),
    }

    // Finalize resource counters.
    if let (Some(c0), s) = (cpu_start, cpu_secs()) {
        let wall = wall_start.elapsed().as_secs_f64();
        if let Some(c1) = s {
            if wall > 0.0 {
                metrics.cpu_percent = Some((c1 - c0) / wall * 100.0);
            }
        }
    }
    metrics.rss_peak_bytes = max_opt(metrics.rss_peak_bytes, rss_bytes());

    let results = Results {
        schema: "ggsl-loadgen/1",
        scenario: scenario.clone(),
        target_name: args.str("target-name", "local"),
        git_sha: args.opt("git-sha").or_else(|| std::env::var("GIT_SHA").ok()).unwrap_or_else(|| "dev".into()),
        timestamp_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0),
        config: ConfigBlock {
            payload_bytes: payload,
            threads,
            rate,
            duration_s,
            count,
            fsync: format!("{fsync:?}"),
            segment_bytes,
            max_disk_bytes,
            on_full: format!("{on_full:?}"),
            sink: sink_str,
            batch_records,
            path: path.to_string_lossy().into_owned(),
        },
        env,
        metrics,
    };

    let json = serde_json::to_string_pretty(&results).expect("serialize results");
    println!("{json}");

    // Persist: --out <file>, else --results-dir/<target>/<sha>.json.
    if let Some(out) = args.opt("out") {
        let _ = fs::write(&out, &json);
        eprintln!("loadgen: wrote {out}");
    } else if let Some(dir) = args.opt("results-dir") {
        let target_dir = Path::new(&dir).join(&results.target_name);
        let _ = fs::create_dir_all(&target_dir);
        let file = target_dir.join(format!("{}-{}.json", results.scenario, results.git_sha));
        let _ = fs::write(&file, &json);
        eprintln!("loadgen: wrote {}", file.display());
    }

    // Clean up the buffer dir unless asked to keep it.
    if !args.flag("keep") {
        let _ = fs::remove_dir_all(&path);
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn max_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// S1/S2/S4/S5/S6/S7/S8: ingest (+ optional export) on a live buffer.
#[allow(clippy::too_many_arguments)]
fn run_streaming(
    scenario: &str,
    buffer: &BufferConfig,
    batch: &BatchConfig,
    delivery: &DeliveryConfig,
    threads: usize,
    payload: usize,
    rate: Option<f64>,
    duration_s: f64,
    count: Option<u64>,
    on_full: OnFull,
    sink_spec: &SinkSpec,
    metrics: &mut Metrics,
) {
    let log = Arc::new(EmbeddedLog::open(buffer.clone()).expect("open buffer"));
    let delivered = Arc::new(AtomicU64::new(0));

    // Export engine (all scenarios except a pure ingest-only S1 still benefit from draining;
    // for S1 the instant CountingSink keeps the buffer from filling so we measure raw ingest).
    let engine = build_sink(sink_spec, Arc::clone(&delivered))
        .map(|sink| ExportEngine::start(Arc::clone(&log), sink, batch.clone(), delivery.clone()));

    // Background sampler: peak RSS, peak backlog, export-lag distribution.
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let log = Arc::clone(&log);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut peak_rss = rss_bytes();
            let mut peak_backlog = 0u64;
            let mut lag = Vec::new();
            while !stop.load(Ordering::Acquire) {
                let s = log.stats();
                peak_backlog = peak_backlog.max(s.backlog);
                lag.push(s.oldest_unacked_age_ms);
                peak_rss = max_opt(peak_rss, rss_bytes());
                std::thread::sleep(Duration::from_millis(20));
            }
            (peak_rss, peak_backlog, lag)
        })
    };

    let per_thread_count = count.map(|c| c / threads as u64);
    let deadline = if count.is_some() { None } else { Some(Instant::now() + Duration::from_secs_f64(duration_s)) };

    let ingest_start = Instant::now();
    let (appended, rejected, mut lat) = run_producers(
        &log, threads, payload, rate, per_thread_count, deadline, on_full,
    );
    let ingest_elapsed = ingest_start.elapsed().as_secs_f64();

    // Drain phase: only the catch-up scenarios (S5 soak / S6 backlog / S7 real-sink) measure
    // time-to-drain. S4 (backpressure/lossy) and S8 (crash) must NOT wait for a full drain — S4's
    // cursor races retention (no meaningful catch-up window) and S8 must crash while backlog exists.
    let measures_drain = matches!(scenario, "S5" | "S6" | "S7");
    let mut drain_rate = None;
    let mut catchup = None;
    if engine.is_some() && measures_drain {
        let target = appended;
        let drain_start = Instant::now();
        // Cap drain wait generously; disconnect scenarios include the outage window.
        let drain_cap = Duration::from_secs_f64((duration_s * 3.0).max(30.0));
        loop {
            let acked = log.acked();
            if acked >= target || drain_start.elapsed() > drain_cap {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let drained = log.acked();
        let drain_elapsed = drain_start.elapsed().as_secs_f64();
        if drain_elapsed > 0.0 {
            drain_rate = Some(drained as f64 / drain_elapsed);
        }
        catchup = Some(drain_elapsed * 1000.0);
    }

    stop.store(true, Ordering::Release);
    let (peak_rss, peak_backlog, lag) = sampler.join().unwrap();

    // S8: simulate crash-resume — drop engine+log without graceful drain, reopen, check resume cost.
    if scenario == "S8" {
        let backlog_at_crash = log.stats().backlog;
        drop(engine);
        drop(log);
        let t0 = Instant::now();
        let reopened = EmbeddedLog::open(buffer.clone()).expect("reopen after crash");
        metrics.recovery_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
        // At-least-once: everything from the checkpoint forward is still readable (no loss).
        let resumable = reopened.read_batch(usize::MAX, usize::MAX).map(|b| b.len()).unwrap_or(0);
        metrics.potential_duplicates = Some(backlog_at_crash.min(resumable as u64));
        metrics.backlog_peak = Some(peak_backlog);
        finalize_common(metrics, appended, delivered.load(Ordering::Relaxed), rejected,
            &reopened, ingest_elapsed, payload, &mut lat, peak_rss, &lag, true);
        return;
    }

    let exported = match &engine {
        Some(e) => e.stats().exported_total.max(delivered.load(Ordering::Relaxed)),
        None => delivered.load(Ordering::Relaxed),
    };
    metrics.drain_rate_rps = drain_rate;
    metrics.time_to_catchup_ms = catchup;
    metrics.backlog_peak = Some(peak_backlog);
    finalize_common(metrics, appended, exported, rejected, &log, ingest_elapsed, payload, &mut lat, peak_rss, &lag, engine.is_some());

    // engine drops here (RAII stop).
    drop(engine);
}

/// Fill in the metrics common to every streaming scenario.
#[allow(clippy::too_many_arguments)]
fn finalize_common(
    m: &mut Metrics,
    appended: u64,
    exported: u64,
    rejected: u64,
    log: &EmbeddedLog,
    ingest_elapsed: f64,
    payload: usize,
    lat: &mut Latencies,
    peak_rss: Option<u64>,
    lag: &[u64],
    engine_ran: bool,
) {
    let s = log.stats();
    m.appended_total = appended;
    m.exported_total = exported;
    m.dropped_total = s.dropped_total;
    m.rejected_total = rejected;
    m.duration_s = ingest_elapsed;
    if ingest_elapsed > 0.0 {
        m.throughput_rps = appended as f64 / ingest_elapsed;
        let bytes = appended as f64 * payload as f64;
        m.throughput_mb_s = bytes / 1e6 / ingest_elapsed;
        let on_disk = appended as f64 * (payload + ggstreamlog::record::FRAME_OVERHEAD) as f64;
        m.logical_write_mb_s = on_disk / 1e6 / ingest_elapsed;
    }
    lat.v.sort_unstable();
    m.append_p50_ns = percentile(&lat.v, 50.0);
    m.append_p99_ns = percentile(&lat.v, 99.0);
    m.append_p999_ns = percentile(&lat.v, 99.9);
    m.append_max_ns = lat.v.last().copied().unwrap_or(0);
    if engine_ran {
        let mut lag_sorted = lag.to_vec();
        lag_sorted.sort_unstable();
        m.export_lag_p99_ms = Some(percentile(&lag_sorted, 99.0));
    }
    m.disk_bytes_final = s.disk_bytes;
    m.rss_peak_bytes = max_opt(peak_rss, rss_bytes());
}

/// S3: recovery time vs log size. Write N records, drop, reopen, measure open() wall-clock.
fn run_recovery(buffer: &BufferConfig, n: u64, payload: usize, metrics: &mut Metrics) {
    {
        let log = EmbeddedLog::open(buffer.clone()).expect("open");
        let pk = "rec";
        let body = vec![b'x'; payload];
        // Batched append keeps the write phase quick; we only time recovery.
        let chunk = 1000usize;
        let mut written = 0u64;
        while written < n {
            let this = chunk.min((n - written) as usize);
            let recs: Vec<Record> =
                (0..this).map(|i| Record::new(pk, 1000 + written + i as u64, body.clone())).collect();
            log.append_batch(&recs).expect("append_batch");
            written += this as u64;
        }
        log.flush().expect("flush");
    }

    let t0 = Instant::now();
    let log = EmbeddedLog::open(buffer.clone()).expect("reopen");
    let recovery_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let s = log.stats();
    assert_eq!(s.next_offset, n, "recovery must restore the full log (next_offset)");

    metrics.recovery_ms = Some(recovery_ms);
    metrics.appended_total = n;
    metrics.disk_bytes_final = s.disk_bytes;
    metrics.rss_peak_bytes = rss_bytes();
    metrics.duration_s = 0.0;
}
