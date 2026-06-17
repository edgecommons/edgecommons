//! # Metrics target — log
//!
//! **One-liner purpose**: Append EMF JSON (one object per line) to a log file with
//! size-based rotation.
//!
//! ## Overview
//! Mirrors the Java/Python `log` metric target. The file path comes from
//! `metricEmission.targetConfig.logFileName` (template-resolved by the emitter).
//! Lines are pure EMF JSON (Python-aligned, so CloudWatch can extract metrics).
//! The file rotates by size like the Java target.
//!
//! ## Semantics & Architecture
//! - Single open file behind a `std::sync::Mutex`; writes are short and never hold
//!   the lock across an `.await`.
//! - **Fail-soft, lazy open** (matching Java's `Log` target): the file is *not*
//!   opened at construction. It is opened on the first emit; if it cannot be opened
//!   (e.g. the default `/greengrass/v2/logs` path is not writable by a non-root
//!   component), a single warning is logged and metrics are dropped rather than
//!   crashing the component. Java's target likewise catches appender-configuration
//!   failures and falls back instead of propagating.
//! - **Rotation**: when a write would exceed `targetConfig.maxFileSize` (default
//!   `10MB`, parsed with 1024-based KB/MB/GB units), the current file is renamed to
//!   `<stem>-<UTC-timestamp><ext>` and a fresh file is opened; up to 5 rolled files
//!   are kept (oldest pruned), matching Java's `DefaultRolloverStrategy(max=5)`.
//! - `large_fleet_workaround` writes a second line with `coreName="ALL"`.
//! - `emit` and `emit_now` behave identically (no batching for a log file).
//! - Error handling: [`crate::error::GgError::Io`] / `Metrics`.
//!
//! ## Related Modules
//! - [`crate::metrics::emf`], [`super`].

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use async_trait::async_trait;

use super::MetricTarget;
use crate::error::{GgError, Result};
use crate::metrics::emf::build_emf_variants;
use crate::metrics::metric::Metric;

/// Default max file size when `maxFileSize` is unset or unparseable.
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Number of rolled files to retain (matches Java's `DefaultRolloverStrategy(max=5)`).
const MAX_BACKUPS: usize = 5;

/// Mutable state behind the lock: the open file, its current byte size, and whether
/// an open failure has already been warned about (so we warn at most once).
struct LogState {
    file: Option<File>,
    size: u64,
    open_warned: bool,
}

/// Appends EMF JSON lines to a file, rotating by size.
pub struct LogTarget {
    path: PathBuf,
    max_bytes: u64,
    namespace: String,
    large_fleet_workaround: bool,
    state: Mutex<LogState>,
}

impl LogTarget {
    /// Construct a log target for `path`, rotating at `max_file_size` (e.g. `"10MB"`).
    ///
    /// The file is **not** opened here — it is opened lazily on the first emit (see
    /// the module docs). Construction is therefore infallible; the `Result` is
    /// retained for call-site/signature compatibility and is always `Ok`.
    #[allow(clippy::unnecessary_wraps)]
    pub fn new(
        path: impl AsRef<Path>,
        namespace: impl Into<String>,
        large_fleet_workaround: bool,
        max_file_size: &str,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        Ok(Self {
            path,
            max_bytes: parse_size(max_file_size).unwrap_or(DEFAULT_MAX_BYTES),
            namespace: namespace.into(),
            large_fleet_workaround,
            state: Mutex::new(LogState {
                file: None,
                size: 0,
                open_warned: false,
            }),
        })
    }

    /// Open (creating parent directories) the metric log file at `self.path`.
    fn open_file(&self) -> std::io::Result<File> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        OpenOptions::new().create(true).append(true).open(&self.path)
    }

    /// Ensure the file is open, opening it lazily on first use. Returns `false`
    /// (fail-soft) if the file cannot be opened, warning at most once.
    fn ensure_open(&self, state: &mut LogState) -> bool {
        if state.file.is_some() {
            return true;
        }
        match self.open_file() {
            Ok(file) => {
                state.size = file.metadata().map(|m| m.len()).unwrap_or(0);
                state.file = Some(file);
                true
            }
            Err(e) => {
                if !state.open_warned {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %e,
                        "metric log: cannot open file; dropping metrics (fail-soft, matching Java)"
                    );
                    state.open_warned = true;
                }
                false
            }
        }
    }

    fn write(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        let variants =
            build_emf_variants(&self.namespace, metric, values, self.large_fleet_workaround);
        let mut state = self
            .state
            .lock()
            .map_err(|_| GgError::Metrics("metric log mutex poisoned".to_string()))?;
        // Lazy, fail-soft open: if the file is unavailable, drop the metric.
        if !self.ensure_open(&mut state) {
            return Ok(());
        }
        for emf in variants {
            let line = serde_json::to_string(&emf)?;
            let needed = line.len() as u64 + 1; // + newline
            if state.size > 0 && state.size + needed > self.max_bytes {
                self.rotate(&mut state)?;
            }
            let file = state
                .file
                .as_mut()
                .ok_or_else(|| GgError::Metrics("metric log file is closed".to_string()))?;
            writeln!(file, "{line}")?;
            state.size += needed;
        }
        Ok(())
    }

    /// Close the current file, rename it to a timestamped backup, prune old backups,
    /// and open a fresh file.
    fn rotate(&self, state: &mut LogState) -> Result<()> {
        if let Some(mut file) = state.file.take() {
            let _ = file.flush();
            // Dropping `file` closes the handle so the rename can succeed on Windows.
        }
        let rolled = self.rolled_path();
        std::fs::rename(&self.path, &rolled)?;
        self.prune_backups();
        let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        state.file = Some(file);
        state.size = 0;
        Ok(())
    }

    /// Compute a unique timestamped backup path: `<stem>-<UTC ts>[ -<n> ]<ext>`.
    fn rolled_path(&self) -> PathBuf {
        let dir = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "metric".to_string());
        let ext = self.path.extension().map(|s| s.to_string_lossy().into_owned());
        let ts = timestamp_compact();

        let build = |suffix: Option<usize>| -> PathBuf {
            let mut name = match suffix {
                Some(n) => format!("{stem}-{ts}-{n}"),
                None => format!("{stem}-{ts}"),
            };
            if let Some(ext) = &ext {
                name.push('.');
                name.push_str(ext);
            }
            dir.join(name)
        };

        let mut candidate = build(None);
        let mut n = 1;
        while candidate.exists() {
            candidate = build(Some(n));
            n += 1;
        }
        candidate
    }

    /// Delete the oldest rolled files beyond [`MAX_BACKUPS`].
    fn prune_backups(&self) {
        let Some(dir) = self.path.parent() else { return };
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = self.path.extension().map(|s| s.to_string_lossy().into_owned());
        let prefix = format!("{stem}-");

        let mut rolled: Vec<(SystemTime, PathBuf)> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .flatten()
                .filter_map(|entry| {
                    let path = entry.path();
                    if path == self.path {
                        return None;
                    }
                    let name = path.file_name()?.to_string_lossy().into_owned();
                    let matches_ext = match &ext {
                        Some(ext) => name.ends_with(&format!(".{ext}")),
                        None => true,
                    };
                    if name.starts_with(&prefix) && matches_ext {
                        let modified = entry.metadata().ok()?.modified().ok()?;
                        Some((modified, path))
                    } else {
                        None
                    }
                })
                .collect(),
            Err(_) => return,
        };

        rolled.sort_by_key(|(modified, _)| *modified); // oldest first
        let excess = rolled.len().saturating_sub(MAX_BACKUPS);
        for (_, path) in rolled.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[async_trait]
impl MetricTarget for LogTarget {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.write(metric, values)
    }

    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()> {
        self.write(metric, values)
    }

    async fn flush(&self) -> Result<()> {
        if let Ok(mut state) = self.state.lock() {
            if let Some(file) = state.file.as_mut() {
                let _ = file.flush();
            }
        }
        Ok(())
    }
}

/// Compact UTC timestamp `YYYYMMDDHHMMSS` for rolled file names.
fn timestamp_compact() -> String {
    let t = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}{:02}{:02}{:02}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Parse a size string like `"10MB"`, `"512KB"`, `"1GB"`, or `"1048576"` into bytes.
/// Units are case-insensitive and 1024-based (matching Log4j2's `FileSize`).
fn parse_size(input: &str) -> Option<u64> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (number, unit) = s.split_at(digits_end);
    let value: u64 = number.trim().parse().ok()?;
    let multiplier = match unit.trim().to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "KB" | "K" => 1024,
        "MB" | "M" => 1024 * 1024,
        "GB" | "G" => 1024 * 1024 * 1024,
        _ => return None,
    };
    Some(value * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::metric::MetricBuilder;

    fn values(n: f64) -> HashMap<String, f64> {
        let mut v = HashMap::new();
        v.insert("count".to_string(), n);
        v
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("10MB"), Some(10 * 1024 * 1024));
        assert_eq!(parse_size("512KB"), Some(512 * 1024));
        assert_eq!(parse_size("1GB"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size("2048"), Some(2048));
        assert_eq!(parse_size("4 mb"), Some(4 * 1024 * 1024));
        assert_eq!(parse_size("nonsense"), None);
    }

    #[tokio::test]
    async fn writes_emf_line_to_file() {
        let dir = std::env::temp_dir().join(format!("ggcommons-log-{}", uuid::Uuid::new_v4()));
        let path = dir.join("metric.log");
        let target = LogTarget::new(&path, "ns", false, "10MB").unwrap();

        let metric = MetricBuilder::create("requests").add_measure("count", "Count", 60).build();
        target.emit_now(&metric, &values(3.0)).await.unwrap();
        target.flush().await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents.lines().next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(parsed["count"], 3.0);
        assert_eq!(parsed["category"], "requests");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn large_fleet_writes_two_lines() {
        let dir = std::env::temp_dir().join(format!("ggcommons-log-lf-{}", uuid::Uuid::new_v4()));
        let path = dir.join("metric.log");
        let target = LogTarget::new(&path, "ns", true, "10MB").unwrap();

        let metric = MetricBuilder::create("m")
            .with_thing_name("thing-1")
            .add_measure("count", "Count", 60)
            .build();
        target.emit_now(&metric, &values(1.0)).await.unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "large fleet emits normal + ALL");
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["coreName"], "thing-1");
        assert_eq!(second["coreName"], "ALL");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unwritable_path_is_fail_soft_not_a_crash() {
        // Use a regular file as a directory component so create_dir_all/open fails.
        let dir = std::env::temp_dir().join(format!("ggcommons-fs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap(); // a file where we'll pretend a dir is
        let path = blocker.join("sub").join("metric.log");

        // Construction must not fail (lazy open).
        let target = LogTarget::new(&path, "ns", false, "10MB").unwrap();
        let metric = MetricBuilder::create("requests").add_measure("count", "Count", 60).build();

        // Emitting must not error even though the file can't be opened (fail-soft).
        target.emit_now(&metric, &values(1.0)).await.unwrap();
        target.flush().await.unwrap();
        assert!(!path.exists(), "no file should have been created");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rotates_when_max_size_exceeded_and_prunes_backups() {
        let dir = std::env::temp_dir().join(format!("ggcommons-rot-{}", uuid::Uuid::new_v4()));
        let path = dir.join("metric.log");
        // Tiny max so each write rotates; keeps at most MAX_BACKUPS rolled files.
        let target = LogTarget::new(&path, "ns", false, "200B").unwrap();
        let metric = MetricBuilder::create("requests").add_measure("count", "Count", 60).build();

        // Each EMF line is well over 200 bytes, so each emit after the first rotates.
        for i in 0..(MAX_BACKUPS + 3) {
            target.emit_now(&metric, &values(i as f64)).await.unwrap();
        }
        target.flush().await.unwrap();

        // Current file exists.
        assert!(path.exists());
        // Rolled backups are capped at MAX_BACKUPS.
        let rolled = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.starts_with("metric-") && n.ends_with(".log")
            })
            .count();
        assert!(rolled <= MAX_BACKUPS, "expected <= {MAX_BACKUPS} backups, got {rolled}");
        assert!(rolled >= 1, "expected at least one rolled backup");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
