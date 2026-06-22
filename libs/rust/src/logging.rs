//! # Logging
//!
//! **One-liner purpose**: Initialize the `tracing` subscriber from config, with
//! runtime-reloadable log level and optional rotating file output.
//!
//! ## Overview
//! Installs a `tracing-subscriber` registry with a console `fmt` layer, a
//! **reloadable** `EnvFilter`, and — when `logging.fileLogging.enabled` is set —
//! a second `fmt` layer writing to a size-rotated file. The level comes from
//! `logging.level`; [`reconfigure`] (driven by a config hot-reload via
//! [`LoggingReconfigurer`]) swaps the filter at runtime.
//!
//! ## Semantics & Architecture
//! - Idempotent install: if a global subscriber already exists, init is a no-op.
//! - The reload handle is stored type-erased in a `OnceLock`; reconfiguration is a
//!   cheap filter swap with no re-init.
//! - **File logging** mirrors the Python library's `RotatingFileHandler`: the file
//!   path is template-resolved, parent directories are created, and the file is
//!   rotated by size (`fileLogging.maxFileSize`, default `10MB`) keeping
//!   `fileLogging.backupCount` (default `5`) backups named `<path>.1`, `<path>.2`,
//!   … The same level filter gates both console and file output.
//! - Error handling: infallible — an unparseable level falls back to `info`; a
//!   file that cannot be opened is reported to stderr and file logging is skipped.
//! - Limitation: the file layer is decided at [`init`] time only (tracing layers
//!   cannot be added/removed after install), so a `fileLogging` change on hot
//!   reload does not take effect until restart; the *level* (root + per-logger
//!   `logging.loggers` overrides) reconfigures live for both sinks. The custom
//!   `logging.format` string is intentionally NOT applied here — a cross-language
//!   format is being addressed via per-language format fields as part of the
//!   shared-configuration work (see .validation/parity-remediation-plan.md #1).
//!
//! ## Usage Example
//! ```
//! use ggcommons::config::model::Config;
//! use ggcommons::logging;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("c", "t", json!({ "logging": { "level": "DEBUG" } })).unwrap();
//! logging::init(&cfg);
//! ```
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`crate::config::model`], [`crate::config::ConfigurationChangeListener`],
//!   [`crate::config::template`].

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, reload, EnvFilter};

use crate::config::model::Config;
use crate::config::template::resolve;
use crate::config::ConfigurationChangeListener;

/// Type-erased "reload the level filter" callback, installed once by [`init`].
static RECONFIGURE: OnceLock<Box<dyn Fn(EnvFilter) + Send + Sync>> = OnceLock::new();

/// Initialize the global tracing subscriber with a reloadable level filter and,
/// if `logging.fileLogging.enabled`, a size-rotated file layer.
pub fn init(config: &Config) {
    let (layer, handle) = reload::Layer::new(level_filter(config));
    // Build the optional file layer inline so its type is inferred against the
    // registry chain. `Option<Layer>` itself implements `Layer`, so a `None`
    // simply adds nothing.
    let file_layer = file_make_writer(config)
        .map(|writer| fmt::layer().with_ansi(false).with_writer(writer));
    let installed = tracing_subscriber::registry()
        .with(layer)
        .with(fmt::layer())
        .with(file_layer)
        .try_init()
        .is_ok();
    if installed {
        let _ = RECONFIGURE.set(Box::new(move |filter: EnvFilter| {
            let _ = handle.reload(filter);
        }));
    }
}

/// Apply the log level from `config` to the running subscriber (no-op if logging
/// was never initialized by this library).
pub fn reconfigure(config: &Config) {
    if let Some(reconfigure) = RECONFIGURE.get() {
        reconfigure(level_filter(config));
    }
}

/// Build an `EnvFilter` from the config's `logging.level` (default `info`) plus any
/// per-logger overrides in `logging.loggers` (each becomes a `target=level` directive),
/// e.g. `info,my::module=debug`. Applied on init and on every hot-reload.
fn level_filter(config: &Config) -> EnvFilter {
    let level = config
        .parsed
        .logging
        .level
        .clone()
        .unwrap_or_else(|| "INFO".to_string());
    let mut directives = level.to_ascii_lowercase();
    for (logger, lvl) in &config.parsed.logging.loggers {
        directives.push_str(&format!(",{}={}", logger, lvl.to_ascii_lowercase()));
    }
    // Fall back to just the root level if a per-logger directive is malformed, then to info.
    EnvFilter::try_new(&directives)
        .or_else(|_| EnvFilter::try_new(level.to_ascii_lowercase()))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Build the rotating-file `MakeWriter` if file logging is enabled and the file
/// can be opened; otherwise `None`. Errors are reported to stderr (the tracing
/// subscriber is not yet installed at this point).
fn file_make_writer(config: &Config) -> Option<RotatingFileMakeWriter> {
    let file_logging = config.parsed.logging.file_logging.as_ref()?;
    if !file_logging.enabled {
        return None;
    }
    let raw_path = file_logging.file_path.as_ref()?;
    let path = PathBuf::from(resolve(config, raw_path));
    let max_bytes = parse_file_size(&file_logging.max_file_size());
    let backup_count = file_logging.backup_count();

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("ggcommons: failed to create log directory {parent:?}: {e}");
                return None;
            }
        }
    }

    match RotatingFileWriter::open(path.clone(), max_bytes, backup_count) {
        Ok(writer) => Some(RotatingFileMakeWriter(Arc::new(Mutex::new(writer)))),
        Err(e) => {
            eprintln!("ggcommons: failed to open log file {path:?}: {e}");
            None
        }
    }
}

/// Parse a size string like `10MB`, `512KB`, `1GB`, or `4096` into bytes.
///
/// Recognizes `B`/`KB`/`MB`/`GB` suffixes (case-insensitive, 1024-based). Falls
/// back to `10MB` when the string cannot be parsed — matching the Python
/// library's `_parse_file_size`.
fn parse_file_size(value: &str) -> u64 {
    const DEFAULT: u64 = 10 * 1024 * 1024;
    let up = value.trim().to_ascii_uppercase();
    // Longer suffixes first so "10MB" is not matched by the bare "B".
    const UNITS: &[(&str, u64)] = &[
        ("KB", 1024),
        ("MB", 1024 * 1024),
        ("GB", 1024 * 1024 * 1024),
        ("B", 1),
    ];
    for (suffix, multiplier) in UNITS {
        if let Some(num) = up.strip_suffix(suffix) {
            if let Ok(v) = num.trim().parse::<u64>() {
                return v.saturating_mul(*multiplier);
            }
        }
    }
    // A bare number (no suffix) is treated as bytes.
    if let Ok(v) = up.parse::<u64>() {
        return v;
    }
    DEFAULT
}

/// A size-rotating file writer: appends to `path`, and when a write would push
/// the file past `max_bytes` it rotates `path` → `path.1`, shifting older
/// backups up to `backup_count` and discarding the oldest. `max_bytes == 0`
/// disables rotation; `backup_count == 0` discards the old file on rollover.
struct RotatingFileWriter {
    path: PathBuf,
    max_bytes: u64,
    backup_count: u64,
    current_size: u64,
    file: Option<File>,
}

impl RotatingFileWriter {
    fn open(path: PathBuf, max_bytes: u64, backup_count: u64) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let current_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            max_bytes,
            backup_count,
            current_size,
            file: Some(file),
        })
    }

    /// `<path>.N` backup name.
    fn backup_path(&self, n: u64) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// Ensure the active file handle is open, returning a mutable reference.
    fn file_mut(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            self.file = Some(OpenOptions::new().create(true).append(true).open(&self.path)?);
        }
        Ok(self.file.as_mut().expect("file just opened"))
    }

    /// Close the active file, shift backups, and reopen a fresh base file.
    fn rotate(&mut self) -> io::Result<()> {
        // Close the active handle first — Windows cannot rename an open file.
        if let Some(mut f) = self.file.take() {
            let _ = f.flush();
        }

        if self.backup_count == 0 {
            // No backups kept: discard the old content.
            let _ = std::fs::remove_file(&self.path);
        } else {
            // Drop the oldest backup, then shift the rest up by one.
            let _ = std::fs::remove_file(self.backup_path(self.backup_count));
            for i in (1..self.backup_count).rev() {
                let src = self.backup_path(i);
                if src.exists() {
                    let _ = std::fs::rename(&src, self.backup_path(i + 1));
                }
            }
            if self.path.exists() {
                let _ = std::fs::rename(&self.path, self.backup_path(1));
            }
        }

        self.file = Some(OpenOptions::new().create(true).append(true).open(&self.path)?);
        self.current_size = 0;
        Ok(())
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Rotate before writing if this write would exceed the limit (but never
        // rotate an empty file, so a single oversized record still lands).
        if self.max_bytes > 0
            && self.current_size > 0
            && self.current_size.saturating_add(buf.len() as u64) > self.max_bytes
        {
            self.rotate()?;
        }
        let n = self.file_mut()?.write(buf)?;
        self.current_size = self.current_size.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// Shareable [`MakeWriter`] over a [`RotatingFileWriter`]. Each log event locks
/// the writer for the duration of the event so writes (and any rotation) are
/// serialized and never interleaved across threads.
#[derive(Clone)]
struct RotatingFileMakeWriter(Arc<Mutex<RotatingFileWriter>>);

/// The per-event writer handle: holds the lock and delegates I/O.
struct LockedFileWriter<'a>(std::sync::MutexGuard<'a, RotatingFileWriter>);

impl Write for LockedFileWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<'a> MakeWriter<'a> for RotatingFileMakeWriter {
    type Writer = LockedFileWriter<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        // Recover from a poisoned lock rather than panicking inside the logger.
        LockedFileWriter(self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// A [`ConfigurationChangeListener`] that re-applies the log level on config hot-reload.
pub struct LoggingReconfigurer;

#[async_trait]
impl ConfigurationChangeListener for LoggingReconfigurer {
    async fn on_configuration_change(&self, config: Arc<Config>) -> bool {
        reconfigure(&config);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ggcommons_logtest_{}_{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn parse_file_size_units() {
        assert_eq!(parse_file_size("10MB"), 10 * 1024 * 1024);
        assert_eq!(parse_file_size("512KB"), 512 * 1024);
        assert_eq!(parse_file_size("1GB"), 1024 * 1024 * 1024);
        assert_eq!(parse_file_size("4096B"), 4096);
        assert_eq!(parse_file_size("4096"), 4096);
        assert_eq!(parse_file_size("  2 mb  "), 2 * 1024 * 1024);
        // Unparseable -> 10MB default (matches Python).
        assert_eq!(parse_file_size("garbage"), 10 * 1024 * 1024);
        assert_eq!(parse_file_size("MB"), 10 * 1024 * 1024);
    }

    #[test]
    fn rotates_and_keeps_backups() {
        let dir = test_dir("rotate_basic");
        let base = dir.join("app.log");
        let mut w = RotatingFileWriter::open(base.clone(), 10, 2).unwrap();

        w.write_all(b"AAAAAAAA").unwrap(); // 8 bytes, under 10
        // 8 + 6 > 10 and file non-empty -> rotate, then write to a fresh file.
        w.write_all(b"BBBBBB").unwrap();
        w.flush().unwrap();

        assert_eq!(read(&base), "BBBBBB");
        assert_eq!(read(&w.backup_path(1)), "AAAAAAAA");

        // Force another rotation: 6 + 6 > 10.
        w.write_all(b"CCCCCC").unwrap();
        w.flush().unwrap();
        assert_eq!(read(&base), "CCCCCC");
        assert_eq!(read(&w.backup_path(1)), "BBBBBB");
        assert_eq!(read(&w.backup_path(2)), "AAAAAAAA");

        // A third rotation must drop the oldest (backup_count = 2).
        w.write_all(b"DDDDDD").unwrap();
        w.flush().unwrap();
        assert_eq!(read(&base), "DDDDDD");
        assert_eq!(read(&w.backup_path(1)), "CCCCCC");
        assert_eq!(read(&w.backup_path(2)), "BBBBBB");
        assert!(!w.backup_path(3).exists(), "backup_count=2 must not keep .3");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_count_zero_truncates() {
        let dir = test_dir("rotate_zero");
        let base = dir.join("app.log");
        let mut w = RotatingFileWriter::open(base.clone(), 10, 0).unwrap();
        w.write_all(b"AAAAAAAA").unwrap();
        w.write_all(b"BBBBBB").unwrap(); // triggers rotation -> old discarded
        w.flush().unwrap();
        assert_eq!(read(&base), "BBBBBB");
        assert!(!w.backup_path(1).exists(), "backup_count=0 keeps no backups");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn max_bytes_zero_disables_rotation() {
        let dir = test_dir("rotate_off");
        let base = dir.join("app.log");
        let mut w = RotatingFileWriter::open(base.clone(), 0, 5).unwrap();
        w.write_all(b"AAAAAAAA").unwrap();
        w.write_all(b"BBBBBB").unwrap();
        w.flush().unwrap();
        assert_eq!(read(&base), "AAAAAAAABBBBBB");
        assert!(!w.backup_path(1).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopen_appends_existing_size() {
        let dir = test_dir("rotate_append");
        let base = dir.join("app.log");
        {
            let mut w = RotatingFileWriter::open(base.clone(), 100, 2).unwrap();
            w.write_all(b"hello").unwrap();
            w.flush().unwrap();
        }
        // Reopen: current_size must reflect the existing 5 bytes so we keep
        // appending rather than starting over.
        let mut w = RotatingFileWriter::open(base.clone(), 100, 2).unwrap();
        assert_eq!(w.current_size, 5);
        w.write_all(b" world").unwrap();
        w.flush().unwrap();
        assert_eq!(read(&base), "hello world");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_layer_built_when_enabled() {
        let dir = test_dir("makewriter");
        let base = dir.join("c.log");
        let cfg = Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({
                "logging": {
                    "level": "INFO",
                    "fileLogging": { "enabled": true, "filePath": base.to_string_lossy() }
                }
            }),
        )
        .unwrap();
        assert!(file_make_writer(&cfg).is_some(), "writer should be built when enabled");

        let cfg_off = Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({ "logging": { "fileLogging": { "enabled": false } } }),
        )
        .unwrap();
        assert!(file_make_writer(&cfg_off).is_none(), "writer should be absent when disabled");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_path_template_is_resolved() {
        let dir = test_dir("template");
        let template = format!("{}/{{ComponentName}}.log", dir.to_string_lossy());
        let cfg = Config::from_value(
            "com.example.MyComp",
            "thing-1",
            serde_json::json!({
                "logging": { "fileLogging": { "enabled": true, "filePath": template } }
            }),
        )
        .unwrap();
        assert!(file_make_writer(&cfg).is_some());
        // The short component name must have been substituted into the path.
        assert!(dir.join("MyComp.log").exists(), "template-resolved log file should be created");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
