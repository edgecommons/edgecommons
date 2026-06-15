//! # Heartbeat
//!
//! **One-liner purpose**: Periodically sample system health and publish it to the
//! metric and/or messaging targets, mirroring the Java/Python heartbeat.
//!
//! ## Overview
//! A background `tokio` task ticks at `heartbeat.intervalSecs` and, for each
//! configured target, either emits the `heartbeat` metric (target `metric`) or
//! publishes a `heartbeat` message (target `messaging`). Stats are collected by
//! [`HeartbeatMonitor`] for the enabled `heartbeat.measures`.
//!
//! ## Semantics & Architecture
//! - The tick body is wrapped so a transient failure logs and the next tick still
//!   fires — the heartbeat can't be permanently killed by one error (unlike the
//!   Java `Timer`-based version), and a missing target `config` is handled.
//! - Stats shape matches Java/Python: a nested object `{ cpu: {cpu_usage}, memory:
//!   {memory_usage}, disk: {disk_total,disk_used,disk_free}, threads: {threads},
//!   files: {files}, fds: {fds} }`. The metric target flattens it to measure→value;
//!   the messaging target sends it as the message payload.
//! - **Measure sources**: cpu/memory/disk via `sysinfo` (all platforms);
//!   threads/fds/files via Linux `/proc` and Windows `windows-sys`
//!   (`GetProcessHandleCount` for fds/files — total handles, as psutil uses
//!   `num_handles`; thread count via a Toolhelp snapshot). On unsupported platforms
//!   an enabled-but-unavailable measure reports `0`.
//! - Reacting to runtime config changes is wired in the config hot-reload increment
//!   (the heartbeat currently uses the snapshot taken at start).
//! - Error handling: per-tick failures are logged, never a panic.
//!
//! ## Usage Example
//! ```no_run
//! # async fn demo(gg: &ggcommons::GgCommons) {
//! // GgCommons starts the heartbeat automatically; it stops when GgCommons is dropped.
//! let _ = gg;
//! # }
//! ```
//!
//! ## Safety & Panics
//! The Windows measure helpers use `unsafe` FFI (`windows-sys`); failures return
//! `None` rather than panicking.
//!
//! ## Related Modules
//! - [`crate::metrics`], [`crate::messaging`].

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::task::JoinHandle;

use crate::config::model::{Config, Measures};
use crate::config::template::resolve;
use crate::messaging::message::MessageBuilder;
use crate::messaging::{MessagingService, Qos};
use crate::metrics::{MetricBuilder, MetricService};

const MESSAGE_NAME: &str = "heartbeat";
const MESSAGE_VERSION: &str = "1.0.0";
const DEFAULT_INTERVAL_SECS: u64 = 5;
const DEFAULT_TOPIC: &str = "ggcommons/{ThingName}/{ComponentName}/heartbeat";
const DEFAULT_DESTINATION: &str = "ipc";

/// Owns the heartbeat background task. Dropping it stops the heartbeat (RAII).
pub struct Heartbeat {
    task: Option<JoinHandle<()>>,
}

impl Heartbeat {
    /// Define the `heartbeat` metric and start the periodic publishing task.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub fn start(config: &Config, metrics: Arc<dyn MetricService>, messaging: Option<Arc<dyn MessagingService>>) -> Heartbeat`
    /// - `messaging` is required only by the `messaging` target; a `messaging`
    ///   target with no service logs a warning and is skipped.
    ///
    /// # Post-conditions
    /// The `heartbeat` metric is defined and a background task is running.
    pub fn start(
        config: &Config,
        metrics: Arc<dyn MetricService>,
        messaging: Option<Arc<dyn MessagingService>>,
    ) -> Heartbeat {
        let interval = config
            .parsed
            .heartbeat
            .interval_secs
            .unwrap_or(DEFAULT_INTERVAL_SECS)
            .max(1);

        // Define the heartbeat metric (all measures, like the Java/Python libs).
        let storage_resolution = if interval < 60 { 1 } else { 60 };
        let metric = MetricBuilder::create("heartbeat")
            .with_config(config)
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

        let config = config.clone();
        let task = tokio::spawn(async move {
            let mut monitor = HeartbeatMonitor::new(config.parsed.heartbeat.measures.clone());
            let mut ticker = tokio::time::interval(Duration::from_secs(interval));
            loop {
                ticker.tick().await;
                let stats = monitor.get_stats();
                publish(&config, &metrics, &messaging, &stats).await;
            }
        });

        tracing::info!(interval_secs = interval, "heartbeat started");
        Heartbeat { task: Some(task) }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Publish `stats` to each configured heartbeat target (best-effort; logs failures).
async fn publish(
    config: &Config,
    metrics: &Arc<dyn MetricService>,
    messaging: &Option<Arc<dyn MessagingService>>,
    stats: &Value,
) {
    for target in &config.parsed.heartbeat.targets {
        match target.target_type.to_ascii_lowercase().as_str() {
            "metric" => {
                let values = flatten(stats);
                if let Err(e) = metrics.emit_metric_now("heartbeat", values).await {
                    tracing::warn!(error = %e, "heartbeat metric emit failed");
                }
            }
            "messaging" => {
                let Some(messaging) = messaging else {
                    tracing::warn!("heartbeat messaging target configured but no messaging service");
                    continue;
                };
                let cfg = target.config.as_ref();
                let topic_template = cfg
                    .and_then(|c| c.get("topic"))
                    .and_then(Value::as_str)
                    .unwrap_or(DEFAULT_TOPIC);
                let topic = resolve(config, topic_template);
                let destination = cfg
                    .and_then(|c| c.get("destination"))
                    .and_then(Value::as_str)
                    .unwrap_or(DEFAULT_DESTINATION);

                let message = MessageBuilder::new(MESSAGE_NAME, MESSAGE_VERSION)
                    .payload(stats.clone())
                    .from_config(config)
                    .build();

                let result = if destination.eq_ignore_ascii_case("iot_core")
                    || destination.eq_ignore_ascii_case("iotcore")
                {
                    messaging
                        .publish_to_iot_core(&topic, &message, Qos::AtLeastOnce)
                        .await
                } else if destination.eq_ignore_ascii_case("ipc")
                    || destination.eq_ignore_ascii_case("local")
                {
                    messaging.publish(&topic, &message).await
                } else {
                    tracing::warn!(destination, "unrecognized heartbeat messaging destination");
                    continue;
                };
                if let Err(e) = result {
                    tracing::warn!(error = %e, "heartbeat publish failed");
                }
            }
            other => tracing::warn!(target = %other, "unknown heartbeat target type"),
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
pub struct HeartbeatMonitor {
    system: sysinfo::System,
    pid: Option<sysinfo::Pid>,
    measures: Measures,
}

impl HeartbeatMonitor {
    /// Create a monitor and take an initial process sample (needed for CPU deltas).
    pub fn new(measures: Measures) -> Self {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::get_current_pid().ok();
        if let Some(pid) = pid {
            system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        }
        Self {
            system,
            pid,
            measures,
        }
    }

    /// Collect the enabled measures as a nested JSON object.
    pub fn get_stats(&mut self) -> Value {
        if let Some(pid) = self.pid {
            self.system
                .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        }
        let process = self.pid.and_then(|pid| self.system.process(pid));

        let mut data = Map::new();
        if self.measures.cpu {
            let cpu = process.map(|p| p.cpu_usage() as f64).unwrap_or(0.0);
            data.insert("cpu".to_string(), json!({ "cpu_usage": cpu }));
        }
        if self.measures.memory {
            let mb = process.map(|p| p.memory() as f64 / 1_000_000.0).unwrap_or(0.0);
            data.insert("memory".to_string(), json!({ "memory_usage": mb }));
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
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
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
}
