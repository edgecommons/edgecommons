//! # Heartbeat
//!
//! **One-liner purpose**: The library-owned UNS `state` keepalive + system-measures
//! metric (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20), mirroring the Java canonical
//! `Heartbeat`.
//!
//! ## Overview
//! A background `tokio` task ticks at `heartbeat.intervalSecs` (default 5 s) and,
//! when `heartbeat.enabled` (the default), publishes each tick:
//! 1. a **`state` keepalive** to `ecv1/{device}/{component}/main/state` — header
//!    name `state`, body `{"status":"RUNNING","uptimeSecs":<n>}` — through the
//!    privileged [`ReservedMessaging`] seam (the `state` class is reserved).
//!    `heartbeat.destination` (`local` | `northbound`) selects the keepalive's
//!    transport only;
//! 2. the enabled system measures (cpu/memory/disk/…) as a metric named
//!    [`SYS_METRIC_NAME`] through the normal metric subsystem (D6 — the measures
//!    keep the metric subsystem's full sink routing).
//!
//! On drop (graceful shutdown) a best-effort `{"status":"STOPPED"}` state is
//! published at most once. The legacy `heartbeat.targets[]` array is removed —
//! hard cut (D-U20).
//!
//! ## Semantics & Architecture
//! - The tick body is wrapped so a transient failure logs and the next tick still
//!   fires; each half (state / metric) is best-effort — a failure in one must not
//!   suppress the other.
//! - The task re-reads the live config snapshot each tick, so hot reloads of
//!   `enabled`/`measures`/`destination` apply immediately and an `intervalSecs`
//!   change rebuilds the ticker.
//! - **Measure sources**: cpu/memory/disk via `sysinfo` (all platforms);
//!   threads/fds/files via Linux `/proc` and Windows `windows-sys`. On unsupported
//!   platforms an enabled-but-unavailable measure reports `0`.
//!
//! ## Related Modules
//! - [`crate::metrics`], [`crate::messaging`], [`crate::uns`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;

use crate::config::model::{Config, Measures};
use crate::messaging::message::MessageBuilder;
use crate::messaging::{Qos, ReservedMessaging};
use crate::metrics::{MetricBuilder, MetricService};
use crate::uns::{Uns, UnsClass};

/// The state keepalive's envelope header name (§4.3).
const STATE_MESSAGE_NAME: &str = "state";
const STATE_MESSAGE_VERSION: &str = "1.0";
/// The metric the heartbeat measures are emitted as (§4.3, D-U20/D6).
pub const SYS_METRIC_NAME: &str = "sys";
const DEFAULT_INTERVAL_SECS: u64 = 5;

/// One component instance's southbound/source connectivity — reported at the INSTANCE LEVEL in the
/// `main` state keepalive's `instances[]`, without minting a separate UNS instance per connection
/// (data + lifecycle stay under `main`; the #1c model). A reference adapter maps each connection to
/// its reachability: OPC UA server session / Modbus slave / file-replicator source directory.
#[derive(Debug, Clone)]
pub struct InstanceConnectivity {
    /// The component instance / connection id.
    pub instance: String,
    /// Whether that instance's southbound/source is currently reachable.
    pub connected: bool,
    /// Optional human detail (endpoint, or the down reason).
    pub detail: Option<String>,
}

impl InstanceConnectivity {
    /// Full constructor.
    pub fn new(instance: impl Into<String>, connected: bool, detail: Option<String>) -> Self {
        Self {
            instance: instance.into(),
            connected,
            detail,
        }
    }

    /// Convenience factory without a detail.
    pub fn of(instance: impl Into<String>, connected: bool) -> Self {
        Self::new(instance, connected, None)
    }

    /// The state-body element `{"instance":…,"connected":…[,"detail":…]}`.
    fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("instance".to_string(), json!(self.instance));
        o.insert("connected".to_string(), json!(self.connected));
        if let Some(d) = &self.detail {
            if !d.trim().is_empty() {
                o.insert("detail".to_string(), json!(d));
            }
        }
        Value::Object(o)
    }
}

/// A component-supplied source of per-instance connectivity, sampled each keepalive tick into the
/// state body's `instances[]`. Register via `gg.set_instance_connectivity_provider(...)`. Keep it
/// cheap and non-blocking (sample a cached status); an empty vec omits the section.
pub type InstanceConnectivityProvider = dyn Fn() -> Vec<InstanceConnectivity> + Send + Sync;

/// The shared, hot-swappable slot holding the optional connectivity provider.
type ConnectivitySlot = RwLock<Option<Arc<InstanceConnectivityProvider>>>;

/// Owns the heartbeat background task. Dropping it stops the heartbeat (RAII) and
/// publishes the best-effort `STOPPED` state (at most once).
pub struct Heartbeat {
    task: Option<JoinHandle<()>>,
    config: Arc<ArcSwap<Config>>,
    reserved: Option<Arc<dyn ReservedMessaging>>,
    /// Ensures the best-effort STOPPED state is published at most once.
    stopped_published: Arc<AtomicBool>,
    /// Monotonic start reference for the keepalive's `uptimeSecs`, shared by the periodic tick
    /// and [`Self::publish_state_now`] (the `_bcast` `republish-state` out-of-band re-emit,
    /// [`crate::uns::RepublishListener`]) so both report the same uptime series.
    start_instant: Instant,
    /// The optional per-instance connectivity provider (#1c), shared with the periodic tick and
    /// [`Self::publish_state_now`]; sampled into each RUNNING state body's `instances[]`.
    connectivity: Arc<ConnectivitySlot>,
}

impl Heartbeat {
    /// Define the [`SYS_METRIC_NAME`] metric and start the periodic keepalive task.
    ///
    /// Crate-private (§4.2): the `state` class is reserved, so the heartbeat
    /// publishes through the crate-private [`ReservedMessaging`] seam handed in by
    /// the runtime builder. `reserved = None` (no messaging transport) skips the
    /// keepalive; the `sys` metric still flows through the metric subsystem.
    pub(crate) fn start(
        config: Arc<ArcSwap<Config>>,
        metrics: Arc<dyn MetricService>,
        reserved: Option<Arc<dyn ReservedMessaging>>,
    ) -> Heartbeat {
        let initial = config.load_full();
        let interval = heartbeat_interval(&initial);

        // Define the sys metric (all measures, like the Java/Python libs).
        let storage_resolution = if interval < 60 { 1 } else { 60 };
        let metric = MetricBuilder::create(SYS_METRIC_NAME)
            .with_config(initial.as_ref())
            .add_measure("disk_total", "Gigabytes", storage_resolution)
            .add_measure("disk_used", "Gigabytes", storage_resolution)
            .add_measure("disk_free", "Gigabytes", storage_resolution)
            .add_measure("cpu_usage", "Percent", storage_resolution)
            .add_measure("memory_usage", "Megabytes", storage_resolution)
            .add_measure("threads", "Count", storage_resolution)
            .add_measure("files", "Count", storage_resolution)
            .add_measure("fds", "Count", storage_resolution)
            .build();
        metrics.define_metric(metric);

        let start_instant = Instant::now();
        let connectivity: Arc<ConnectivitySlot> = Arc::new(RwLock::new(None));
        let task_connectivity = connectivity.clone();
        let task_config = config.clone();
        let task_reserved = reserved.clone();
        let initial_measures = initial.parsed.heartbeat.measures.clone();
        let task = tokio::spawn(async move {
            let mut monitor = HeartbeatMonitor::new(initial_measures);
            let mut current_interval = interval;
            let mut ticker = tokio::time::interval(Duration::from_secs(current_interval));
            loop {
                ticker.tick().await;
                let cfg = task_config.load_full();

                // React to an interval change by rebuilding the ticker.
                let new_interval = heartbeat_interval(&cfg);
                if new_interval != current_interval {
                    current_interval = new_interval;
                    ticker = tokio::time::interval(Duration::from_secs(current_interval));
                    ticker.tick().await; // consume the immediate first tick
                }

                if !cfg.parsed.heartbeat.enabled {
                    continue; // disabled by config (hot-reloadable) — keep ticking
                }

                // §4.3: each half is best-effort — a failure in one must not
                // suppress the other.
                if let Some(reserved) = &task_reserved {
                    let conns = task_connectivity.read().unwrap().clone();
                    publish_state(
                        &cfg,
                        reserved.as_ref(),
                        "RUNNING",
                        Some(start_instant.elapsed().as_secs()),
                        conns,
                    )
                    .await;
                }
                monitor.set_measures(cfg.parsed.heartbeat.measures.clone());
                let stats = monitor.get_stats();
                let values = flatten(&stats);
                if let Err(e) = metrics.emit_metric_now(SYS_METRIC_NAME, values).await {
                    tracing::warn!(error = %e, "heartbeat '{SYS_METRIC_NAME}' metric emit failed");
                }
            }
        });

        tracing::info!(
            interval_secs = interval,
            enabled = initial.parsed.heartbeat.enabled,
            destination = initial.parsed.heartbeat.destination(),
            "heartbeat started (UNS state keepalive + sys metric)"
        );
        Heartbeat {
            task: Some(task),
            config,
            reserved,
            stopped_published: Arc::new(AtomicBool::new(false)),
            start_instant,
            connectivity,
        }
    }

    /// Re-emits the RUNNING `state` keepalive immediately, out of band from the periodic
    /// schedule — the `republish-state` broadcast re-announce
    /// ([`crate::uns::RepublishListener`], DESIGN-uns §9.3/§9.4, the late-join lever): the exact
    /// tick payload (`{"status":"RUNNING","uptimeSecs":n}`), the same [`ReservedMessaging`]
    /// seam, the same `heartbeat.destination` routing.
    ///
    /// **Respects `heartbeat.enabled`**: a component whose operator disabled the state
    /// keepalive does not re-announce state — the broadcast cannot re-enable an opted-out state
    /// surface. Best-effort: failures are logged and swallowed (via [`publish_state`]); with no
    /// messaging seam (no transport) this is a silent no-op.
    pub(crate) async fn publish_state_now(&self) {
        let Some(reserved) = self.reserved.as_deref() else {
            return;
        };
        let cfg = self.config.load_full();
        if !cfg.parsed.heartbeat.enabled {
            tracing::debug!(
                "republish-state re-announce skipped: the heartbeat state keepalive is disabled \
                 (heartbeat.enabled=false)"
            );
            return;
        }
        let conns = self.connectivity.read().unwrap().clone();
        publish_state(
            &cfg,
            reserved,
            "RUNNING",
            Some(self.start_instant.elapsed().as_secs()),
            conns,
        )
        .await;
    }

    /// Registers (or clears with `None`) the per-instance connectivity provider whose result is
    /// emitted in each RUNNING `state` keepalive's `instances[]` — the overridable surface a
    /// multi-connection component uses to report connectivity at the instance level without a
    /// separate UNS instance per connection. Wired from `EdgeCommons::set_instance_connectivity_provider`.
    pub(crate) fn set_instance_connectivity_provider(
        &self,
        provider: Option<Arc<InstanceConnectivityProvider>>,
    ) {
        *self.connectivity.write().unwrap() = provider;
    }

    /// The heartbeat's monotonic uptime in seconds (Java: `Heartbeat.getUptimeSecs()`) — the
    /// `ping` command verb's uptime source (DESIGN-uns §9.5,
    /// [`crate::commands::CommandInbox`]), the same series the state keepalive reports.
    /// Available even when `heartbeat.enabled` is `false` (the ticks stop, but the monotonic
    /// clock keeps running): `ping` must always answer, proving the component is responsive
    /// to addressed commands independent of the (possibly disabled) keepalive.
    pub(crate) fn uptime_secs(&self) -> u64 {
        self.start_instant.elapsed().as_secs()
    }
}

/// The configured heartbeat interval in seconds (default 5, minimum 1).
fn heartbeat_interval(config: &Config) -> u64 {
    config
        .parsed
        .heartbeat
        .interval_secs
        .unwrap_or(DEFAULT_INTERVAL_SECS)
        .max(1)
}

/// Publishes one `state` envelope to the component's UNS state topic through the
/// privileged seam (§4.3). Best-effort: failures are logged, never propagated.
///
/// `uptime_secs = Some(n)` is the RUNNING keepalive shape
/// (`{"status":"RUNNING","uptimeSecs":n}`); `None` omits `uptimeSecs` (the STOPPED
/// shape — pinned by the `uns-test-vectors` golden envelopes).
async fn publish_state(
    config: &Config,
    reserved: &dyn ReservedMessaging,
    status: &str,
    uptime_secs: Option<u64>,
    connectivity: Option<Arc<InstanceConnectivityProvider>>,
) {
    // The RAW includeRoot flag (Java parity): Uns applies D-U25 internally.
    let uns = Uns::new(config.identity().clone(), config.topic_include_root());
    let topic = match uns.topic(UnsClass::State) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "heartbeat state keepalive failed to build its UNS topic");
            return;
        }
    };
    let mut body = Map::new();
    body.insert("status".to_string(), json!(status));
    if let Some(uptime) = uptime_secs {
        body.insert("uptimeSecs".to_string(), json!(uptime));
    }
    // Per-instance connectivity — the state body's instances[] (RUNNING keepalive only). Best-effort:
    // catch a panicking provider so a provider bug can never suppress the keepalive itself.
    if uptime_secs.is_some() {
        if let Some(provider) = connectivity {
            let items = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| provider()))
                .unwrap_or_else(|_| {
                    tracing::warn!(
                        "instance connectivity provider panicked; omitting instances[] this tick"
                    );
                    Vec::new()
                });
            if !items.is_empty() {
                let arr: Vec<Value> = items.iter().map(InstanceConnectivity::to_json).collect();
                body.insert("instances".to_string(), Value::Array(arr));
            }
        }
    }
    let message = MessageBuilder::new(STATE_MESSAGE_NAME, STATE_MESSAGE_VERSION)
        .payload(Value::Object(body))
        .from_config(config)
        .build();

    let destination = config.parsed.heartbeat.destination();
    let result = if destination.eq_ignore_ascii_case("northbound") {
        reserved
            .publish_reserved_northbound(&topic, &message, Qos::AtLeastOnce)
            .await
    } else {
        reserved.publish_reserved(&topic, &message).await
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, topic = %topic, "heartbeat state keepalive publish failed");
    }
}

impl Drop for Heartbeat {
    /// Stops the periodic task and publishes the best-effort
    /// `{"status":"STOPPED"}` state (§4.3/D-U14 — at most once; spawned
    /// fire-and-forget when a Tokio runtime is available, and skipped when the
    /// heartbeat is disabled or no messaging seam exists).
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let Some(reserved) = self.reserved.clone() else {
            return;
        };
        let config = self.config.load_full();
        if !config.parsed.heartbeat.enabled {
            return;
        }
        if self.stopped_published.swap(true, Ordering::SeqCst) {
            return; // already published
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                publish_state(&config, reserved.as_ref(), "STOPPED", None, None).await;
            });
        }
    }
}

/// Flatten the nested stats object into a flat `measure -> value` map.
fn flatten(stats: &Value) -> std::collections::HashMap<String, f64> {
    let mut out = std::collections::HashMap::new();
    if let Some(categories) = stats.as_object() {
        for category in categories.values() {
            if let Some(measures) = category.as_object() {
                for (name, value) in measures {
                    if let Some(n) = value.as_f64() {
                        out.insert(name.clone(), n);
                    }
                }
            }
        }
    }
    out
}

/// Collects system health statistics for the enabled measures.
///
/// CPU usage is measured as a delta between consecutive [`get_stats`](Self::get_stats)
/// calls, so the value is meaningful only when the calls are spaced by a real
/// interval (the heartbeat period). The first sample has no baseline and therefore
/// reports `0.0`, matching psutil's first `cpu_percent()` call. The reported value
/// follows the sysinfo convention where `100%` is one fully-used core (it can exceed
/// 100% for a multi-threaded process).
pub struct HeartbeatMonitor {
    system: sysinfo::System,
    pid: Option<sysinfo::Pid>,
    measures: Measures,
    /// False until the first sample establishes a CPU baseline.
    primed: bool,
}

impl HeartbeatMonitor {
    /// Create a monitor. The first [`get_stats`](Self::get_stats) call establishes the
    /// CPU baseline (and reports CPU as `0.0`); later calls measure over the interval.
    pub fn new(measures: Measures) -> Self {
        Self {
            system: sysinfo::System::new(),
            pid: sysinfo::get_current_pid().ok(),
            measures,
            primed: false,
        }
    }

    /// Update which measures are collected (used when config is hot-reloaded).
    pub fn set_measures(&mut self, measures: Measures) {
        self.measures = measures;
    }

    /// Collect the enabled measures as a nested JSON object.
    pub fn get_stats(&mut self) -> Value {
        if let Some(pid) = self.pid {
            self.system
                .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        }
        // The first refresh has no prior sample, so its CPU delta is meaningless.
        let was_primed = self.primed;
        self.primed = true;

        let (cpu_usage, memory_mb) = match self.pid.and_then(|pid| self.system.process(pid)) {
            Some(process) => (
                process.cpu_usage() as f64,
                process.memory() as f64 / 1_000_000.0,
            ),
            None => (0.0, 0.0),
        };

        let mut data = Map::new();
        if self.measures.cpu {
            let cpu = if was_primed { cpu_usage } else { 0.0 };
            data.insert("cpu".to_string(), json!({ "cpu_usage": cpu }));
        }
        if self.measures.memory {
            data.insert("memory".to_string(), json!({ "memory_usage": memory_mb }));
        }
        if self.measures.disk {
            let (total, used, free) = disk_usage_gb();
            data.insert(
                "disk".to_string(),
                json!({ "disk_total": total, "disk_used": used, "disk_free": free }),
            );
        }
        if self.measures.threads {
            data.insert(
                "threads".to_string(),
                json!({ "threads": thread_count().unwrap_or(0) }),
            );
        }
        if self.measures.files {
            data.insert(
                "files".to_string(),
                json!({ "files": open_file_count().unwrap_or(0) }),
            );
        }
        if self.measures.fds {
            data.insert("fds".to_string(), json!({ "fds": fd_count().unwrap_or(0) }));
        }
        Value::Object(data)
    }
}

/// Disk total/used/free in gigabytes for the filesystem holding the current dir.
fn disk_usage_gb() -> (f64, f64, f64) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let cwd = std::env::current_dir().unwrap_or_default();

    // Prefer the disk whose mount point is the longest prefix of the current dir.
    let by_mount = disks
        .list()
        .iter()
        .filter(|d| cwd.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());
    let disk = by_mount.or_else(|| disks.list().iter().max_by_key(|d| d.total_space()));

    match disk {
        Some(d) => {
            let total = d.total_space() as f64;
            let avail = d.available_space() as f64;
            (total / 1e9, (total - avail) / 1e9, avail / 1e9)
        }
        None => (0.0, 0.0, 0.0),
    }
}

// ----- Platform-specific process counters -----

#[cfg(target_os = "linux")]
fn thread_count() -> Option<u64> {
    std::fs::read_dir("/proc/self/task")
        .ok()
        .map(|entries| entries.flatten().count() as u64)
}

#[cfg(target_os = "linux")]
fn fd_count() -> Option<u64> {
    std::fs::read_dir("/proc/self/fd")
        .ok()
        .map(|entries| entries.flatten().count() as u64)
}

#[cfg(target_os = "linux")]
fn open_file_count() -> Option<u64> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    let mut count = 0;
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path()) {
            if target.is_file() {
                count += 1;
            }
        }
    }
    Some(count)
}

#[cfg(windows)]
fn thread_count() -> Option<u64> {
    windows_counters::thread_count()
}

#[cfg(windows)]
fn fd_count() -> Option<u64> {
    windows_counters::handle_count()
}

#[cfg(windows)]
fn open_file_count() -> Option<u64> {
    // Windows has no cheap "open regular files" count; report total handles
    // (matches psutil using num_handles for the fds measure).
    windows_counters::handle_count()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn thread_count() -> Option<u64> {
    None
}
#[cfg(not(any(target_os = "linux", windows)))]
fn fd_count() -> Option<u64> {
    None
}
#[cfg(not(any(target_os = "linux", windows)))]
fn open_file_count() -> Option<u64> {
    None
}

#[cfg(windows)]
mod windows_counters {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount,
    };

    /// Number of open handles for the current process (used for fds/files).
    pub fn handle_count() -> Option<u64> {
        // SAFETY: GetProcessHandleCount writes a u32 through the out-pointer; the
        // pseudo-handle from GetCurrentProcess needs no closing.
        unsafe {
            let mut count: u32 = 0;
            if GetProcessHandleCount(GetCurrentProcess(), &mut count) != 0 {
                Some(count as u64)
            } else {
                None
            }
        }
    }

    /// Number of threads owned by the current process (via a Toolhelp snapshot).
    pub fn thread_count() -> Option<u64> {
        // SAFETY: snapshot is validated against INVALID_HANDLE_VALUE and closed;
        // the THREADENTRY32 is zero-initialized with dwSize set as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return None;
            }
            let pid = GetCurrentProcessId();
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;

            let mut count: u64 = 0;
            if Thread32First(snapshot, &mut entry) != 0 {
                loop {
                    if entry.th32OwnerProcessID == pid {
                        count += 1;
                    }
                    if Thread32Next(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
            Some(count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{RecordingMessaging, RecordingMetrics};
    use serde_json::json;

    fn all_measures() -> Measures {
        Measures {
            cpu: true,
            memory: true,
            disk: true,
            threads: true,
            files: true,
            fds: true,
        }
    }

    fn heartbeat_config(heartbeat: Value) -> Config {
        Config::from_value(
            "com.example.MyComp",
            "thing-1",
            json!({ "heartbeat": heartbeat }),
        )
        .unwrap()
    }

    #[test]
    fn monitor_collects_enabled_measures() {
        let mut monitor = HeartbeatMonitor::new(all_measures());
        let stats = monitor.get_stats();

        // Memory should be a positive number of MB for this live process.
        assert!(stats["memory"]["memory_usage"].as_f64().unwrap() > 0.0);
        assert!(stats["cpu"]["cpu_usage"].is_number());
        assert!(stats["disk"]["disk_total"].as_f64().unwrap() >= 0.0);
        // threads/fds are platform-backed on Linux and Windows.
        #[cfg(any(target_os = "linux", windows))]
        {
            assert!(stats["threads"]["threads"].as_u64().unwrap() >= 1);
            assert!(stats["fds"]["fds"].as_u64().unwrap() >= 1);
        }
    }

    #[test]
    fn disabled_measures_are_omitted() {
        let measures = Measures {
            cpu: true,
            memory: false,
            disk: false,
            threads: false,
            files: false,
            fds: false,
        };
        let mut monitor = HeartbeatMonitor::new(measures);
        let stats = monitor.get_stats();
        assert!(stats.get("cpu").is_some());
        assert!(stats.get("memory").is_none());
        assert!(stats.get("disk").is_none());
    }

    #[test]
    fn flatten_collapses_nested_stats() {
        let stats = json!({
            "cpu": { "cpu_usage": 1.5 },
            "memory": { "memory_usage": 10.0 },
            "disk": { "disk_total": 100.0, "disk_used": 40.0, "disk_free": 60.0 }
        });
        let flat = flatten(&stats);
        assert_eq!(flat.get("cpu_usage"), Some(&1.5));
        assert_eq!(flat.get("memory_usage"), Some(&10.0));
        assert_eq!(flat.get("disk_free"), Some(&60.0));
        assert_eq!(flat.len(), 5);
    }

    /// One RUNNING keepalive publish: the UNS state topic, the pinned envelope
    /// header, the `{status, uptimeSecs}` body shape, and the identity element.
    #[tokio::test]
    async fn publish_state_running_shape_and_topic() {
        let config = heartbeat_config(json!({ "intervalSecs": 1 }));
        let recorder = RecordingMessaging::new();

        publish_state(&config, recorder.as_ref(), "RUNNING", Some(42), None).await;

        let published = recorder.reserved_local();
        assert_eq!(published.len(), 1, "one state keepalive through the seam");
        let (topic, msg) = &published[0];
        assert_eq!(topic, "ecv1/thing-1/MyComp/main/state");
        assert_eq!(msg.header.name, "state");
        assert_eq!(msg.header.version, "1.0");
        assert_eq!(msg.body["status"], "RUNNING");
        assert_eq!(msg.body["uptimeSecs"], 42);
        let identity = msg
            .identity
            .as_ref()
            .expect("state envelope carries identity");
        assert_eq!(identity.device(), "thing-1");
        assert_eq!(identity.instance(), "main");
        assert!(
            recorder.reserved_iot().is_empty(),
            "local destination must not hit IoT Core"
        );
        assert!(
            recorder.local().is_empty(),
            "the keepalive must use the SEAM, not publish()"
        );
    }

    /// The #1c per-instance connectivity surface: a provider's result rides the RUNNING state
    /// body's `instances[]`, with the detail omitted when absent.
    #[tokio::test]
    async fn publish_state_carries_per_instance_connectivity() {
        let config = heartbeat_config(json!({ "intervalSecs": 1 }));
        let recorder = RecordingMessaging::new();
        let provider: Arc<InstanceConnectivityProvider> = Arc::new(|| {
            vec![
                InstanceConnectivity::new("filler1", true, Some("opc.tcp://kep:49320".to_string())),
                InstanceConnectivity::of("kep2", false),
            ]
        });

        publish_state(
            &config,
            recorder.as_ref(),
            "RUNNING",
            Some(1),
            Some(provider),
        )
        .await;

        let published = recorder.reserved_local();
        let instances = published[0].1.body["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0]["instance"], "filler1");
        assert_eq!(instances[0]["connected"], true);
        assert_eq!(instances[0]["detail"], "opc.tcp://kep:49320");
        assert_eq!(instances[1]["instance"], "kep2");
        assert_eq!(instances[1]["connected"], false);
        assert!(instances[1].get("detail").is_none(), "no detail -> omitted");
    }

    /// No provider / an empty result / the STOPPED state all omit the `instances[]` section.
    #[tokio::test]
    async fn no_provider_empty_or_stopped_omits_instances() {
        let config = heartbeat_config(json!({ "intervalSecs": 1 }));

        let r1 = RecordingMessaging::new();
        publish_state(&config, r1.as_ref(), "RUNNING", Some(1), None).await;
        assert!(r1.reserved_local()[0].1.body.get("instances").is_none());

        let r2 = RecordingMessaging::new();
        let empty: Arc<InstanceConnectivityProvider> = Arc::new(Vec::new);
        publish_state(&config, r2.as_ref(), "RUNNING", Some(1), Some(empty)).await;
        assert!(r2.reserved_local()[0].1.body.get("instances").is_none());

        let r3 = RecordingMessaging::new();
        let p: Arc<InstanceConnectivityProvider> =
            Arc::new(|| vec![InstanceConnectivity::of("x", true)]);
        publish_state(&config, r3.as_ref(), "STOPPED", None, Some(p)).await;
        assert!(
            r3.reserved_local()[0].1.body.get("instances").is_none(),
            "STOPPED carries no instances"
        );
    }

    /// Best-effort: a panicking provider omits `instances[]` but never suppresses the keepalive.
    #[tokio::test]
    async fn a_panicking_provider_never_suppresses_the_keepalive() {
        let config = heartbeat_config(json!({ "intervalSecs": 1 }));
        let recorder = RecordingMessaging::new();
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // silence the expected panic print
        let provider: Arc<InstanceConnectivityProvider> = Arc::new(|| panic!("boom"));
        publish_state(
            &config,
            recorder.as_ref(),
            "RUNNING",
            Some(1),
            Some(provider),
        )
        .await;
        std::panic::set_hook(prev);

        let published = recorder.reserved_local();
        assert_eq!(
            published.len(),
            1,
            "a panicking provider must not suppress the keepalive"
        );
        assert_eq!(published[0].1.body["status"], "RUNNING");
        assert!(published[0].1.body.get("instances").is_none());
    }

    #[test]
    fn instance_connectivity_serializes() {
        assert_eq!(
            InstanceConnectivity::new("plc1", true, Some("tcp://10.0.0.50:502".to_string()))
                .to_json(),
            json!({ "instance": "plc1", "connected": true, "detail": "tcp://10.0.0.50:502" })
        );
        assert_eq!(
            InstanceConnectivity::of("plc1", false).to_json(),
            json!({ "instance": "plc1", "connected": false })
        );
        assert!(
            InstanceConnectivity::new("plc1", false, Some("  ".to_string()))
                .to_json()
                .get("detail")
                .is_none(),
            "blank detail -> omitted"
        );
    }

    /// `publish_state_now` (the `_bcast` `republish-state` out-of-band re-emit action wired
    /// into [`crate::uns::RepublishListener`]) re-emits a RUNNING keepalive on demand, through
    /// the same seam/topic/shape as a periodic tick.
    #[tokio::test]
    async fn publish_state_now_re_emits_the_running_keepalive() {
        // A long interval keeps the periodic task's own tick out of the assertion window.
        let config = heartbeat_config(json!({ "intervalSecs": 60 }));
        let recorder = RecordingMessaging::new();
        let metrics: Arc<dyn MetricService> = RecordingMetrics::new();
        let config_handle = Arc::new(ArcSwap::from_pointee(config));
        let hb = Heartbeat::start(config_handle, metrics, Some(recorder.clone()));

        hb.publish_state_now().await;

        let states = recorder.reserved_local();
        assert!(
            !states.is_empty(),
            "at least the out-of-band RUNNING keepalive"
        );
        let (topic, msg) = states.last().unwrap();
        assert_eq!(topic, "ecv1/thing-1/MyComp/main/state");
        assert_eq!(msg.header.name, "state");
        assert_eq!(msg.body["status"], "RUNNING");
        assert!(msg.body.get("uptimeSecs").is_some());
    }

    /// `heartbeat.enabled: false` -> `publish_state_now` is a no-op: the broadcast cannot
    /// re-enable an opted-out state surface.
    #[tokio::test]
    async fn publish_state_now_respects_heartbeat_disabled() {
        let config = heartbeat_config(json!({ "enabled": false, "intervalSecs": 60 }));
        let recorder = RecordingMessaging::new();
        let metrics: Arc<dyn MetricService> = RecordingMetrics::new();
        let config_handle = Arc::new(ArcSwap::from_pointee(config));
        let hb = Heartbeat::start(config_handle, metrics, Some(recorder.clone()));

        hb.publish_state_now().await;

        assert!(recorder.reserved_local().is_empty());
    }

    /// No messaging seam (no transport): `publish_state_now` is a silent no-op, not a panic.
    #[tokio::test]
    async fn publish_state_now_without_messaging_is_a_noop() {
        let config = heartbeat_config(json!({ "intervalSecs": 60 }));
        let metrics: Arc<dyn MetricService> = RecordingMetrics::new();
        let config_handle = Arc::new(ArcSwap::from_pointee(config));
        let hb = Heartbeat::start(config_handle, metrics, None);

        hb.publish_state_now().await; // must not panic
    }

    /// The STOPPED shape omits uptimeSecs (pinned by the golden envelopes).
    #[tokio::test]
    async fn publish_state_stopped_omits_uptime() {
        let config = heartbeat_config(json!({}));
        let recorder = RecordingMessaging::new();
        publish_state(&config, recorder.as_ref(), "STOPPED", None, None).await;
        let (_, msg) = &recorder.reserved_local()[0];
        assert_eq!(msg.body["status"], "STOPPED");
        assert!(msg.body.get("uptimeSecs").is_none());
    }

    /// `heartbeat.destination: northbound` routes the keepalive to the northbound broker.
    #[tokio::test]
    async fn state_destination_northbound_routes_to_iot_core_api() {
        let config = heartbeat_config(json!({ "destination": "northbound" }));
        let recorder = RecordingMessaging::new();
        publish_state(&config, recorder.as_ref(), "RUNNING", Some(1), None).await;
        assert!(recorder.reserved_local().is_empty());
        assert_eq!(recorder.reserved_iot().len(), 1);
        assert_eq!(
            recorder.reserved_iot()[0].0,
            "ecv1/thing-1/MyComp/main/state"
        );
    }

    /// includeRoot with a multi-level hierarchy prepends the site level (D-U25).
    #[tokio::test]
    async fn state_topic_carries_effective_root() {
        let config = Config::from_value(
            "com.example.MyComp",
            "gw-01",
            json!({
                "topic": { "includeRoot": true },
                "hierarchy": { "levels": ["site", "device"] },
                "identity": { "site": "dallas" }
            }),
        )
        .unwrap();
        let recorder = RecordingMessaging::new();
        publish_state(&config, recorder.as_ref(), "RUNNING", Some(1), None).await;
        assert_eq!(
            recorder.reserved_local()[0].0,
            "ecv1/dallas/gw-01/MyComp/main/state"
        );
    }

    /// The running heartbeat publishes the state keepalive AND emits the `sys`
    /// metric on its interval.
    #[tokio::test]
    async fn heartbeat_ticks_state_and_sys_metric() {
        let config = heartbeat_config(json!({ "intervalSecs": 1, "measures": { "cpu": true } }));
        let recorder = RecordingMessaging::new();
        let metrics_recorder = RecordingMetrics::new();
        let metrics: Arc<dyn MetricService> = metrics_recorder.clone();
        let config_handle = Arc::new(ArcSwap::from_pointee(config));

        let hb = Heartbeat::start(config_handle, metrics, Some(recorder.clone()));

        for _ in 0..40 {
            if recorder.reserved_local().len() >= 2 && metrics_recorder.emissions().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let states = recorder.reserved_local();
        assert!(
            states.len() >= 2,
            "expected >=2 keepalives, got {}",
            states.len()
        );
        assert!(
            states
                .iter()
                .all(|(t, _)| t == "ecv1/thing-1/MyComp/main/state")
        );
        assert!(states.iter().all(|(_, m)| m.body["status"] == "RUNNING"));
        // uptimeSecs is present and non-decreasing.
        let uptimes: Vec<u64> = states
            .iter()
            .map(|(_, m)| m.body["uptimeSecs"].as_u64().unwrap())
            .collect();
        assert!(
            uptimes.windows(2).all(|w| w[0] <= w[1]),
            "uptime must not decrease: {uptimes:?}"
        );

        let emissions = metrics_recorder.emissions();
        assert!(emissions.len() >= 2);
        let (name, values) = &emissions[0];
        assert_eq!(name, SYS_METRIC_NAME);
        assert!(values.contains_key("cpu_usage"));

        // Dropping publishes the best-effort STOPPED state (at most once).
        drop(hb);
        for _ in 0..40 {
            if recorder
                .reserved_local()
                .iter()
                .any(|(_, m)| m.body["status"] == "STOPPED")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let stopped: Vec<_> = recorder
            .reserved_local()
            .into_iter()
            .filter(|(_, m)| m.body["status"] == "STOPPED")
            .collect();
        assert_eq!(stopped.len(), 1, "exactly one best-effort STOPPED state");
        assert!(
            stopped[0].1.body.get("uptimeSecs").is_none(),
            "STOPPED omits uptimeSecs"
        );
    }

    /// `heartbeat.enabled: false` publishes nothing (and drop publishes no STOPPED).
    #[tokio::test]
    async fn disabled_heartbeat_publishes_nothing() {
        let config = heartbeat_config(json!({ "enabled": false, "intervalSecs": 1 }));
        let recorder = RecordingMessaging::new();
        let metrics: Arc<dyn MetricService> = RecordingMetrics::new();
        let config_handle = Arc::new(ArcSwap::from_pointee(config));

        let hb = Heartbeat::start(config_handle, metrics, Some(recorder.clone()));
        tokio::time::sleep(Duration::from_millis(1300)).await;
        assert!(recorder.reserved_local().is_empty());
        assert!(recorder.reserved_iot().is_empty());
        drop(hb);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            recorder.reserved_local().is_empty(),
            "no STOPPED when disabled"
        );
    }

    /// No messaging seam (no transport) — the sys metric still flows.
    #[tokio::test]
    async fn no_messaging_still_emits_sys_metric() {
        let config = heartbeat_config(json!({ "intervalSecs": 1, "measures": { "cpu": true } }));
        let metrics_recorder = RecordingMetrics::new();
        let metrics: Arc<dyn MetricService> = metrics_recorder.clone();
        let config_handle = Arc::new(ArcSwap::from_pointee(config));

        let _hb = Heartbeat::start(config_handle, metrics, None);
        for _ in 0..40 {
            if !metrics_recorder.emissions().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            !metrics_recorder.emissions().is_empty(),
            "sys metric emitted without messaging"
        );
    }

    /// Raising `intervalSecs` via hot-reload rebuilds the ticker so later keepalives
    /// are spaced by the new interval.
    #[tokio::test]
    async fn heartbeat_reacts_to_interval_change() {
        let config = heartbeat_config(json!({ "intervalSecs": 1, "measures": { "cpu": true } }));
        let recorder = RecordingMessaging::new();
        let metrics: Arc<dyn MetricService> = RecordingMetrics::new();
        let handle = Arc::new(ArcSwap::from_pointee(config));

        let _hb = Heartbeat::start(handle.clone(), metrics, Some(recorder.clone()));

        // Let a couple of 1s ticks happen, then widen the interval to 3s.
        for _ in 0..25 {
            if recorder.times().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        handle.store(Arc::new(heartbeat_config(
            json!({ "intervalSecs": 3, "measures": { "cpu": true } }),
        )));
        let before = recorder.times().len();
        // Within ~1.5s the old 1s cadence would have added multiple keepalives; the
        // new 3s cadence should add at most one.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let added = recorder.times().len() - before;
        assert!(
            added <= 1,
            "after widening to 3s, expected <=1 new keepalive in 1.5s, got {added}"
        );
    }
}
