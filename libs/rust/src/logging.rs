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
//! - **Format / sink selection** (FR-LOG-4) is decided once at [`init`] from the
//!   *effective format*: explicit `logging.rust_format` ▸ the platform-profile default
//!   (`json` on KUBERNETES) ▸ `None` (the library default). Three sinks:
//!   - `"json"` (case-insensitive) → the structured **stdout-JSON** sink ([`JsonLayer`],
//!     FR-LOG-1): one JSON object per line on stdout, **stdout-only** — no in-process file
//!     rotation is installed (FR-LOG-2), so a read-only root FS never breaks logging. Best-effort
//!     correlation fields (`pod`/`namespace`/`node`/`thing`) are added when present (FR-LOG-3).
//!   - any other token (`{timestamp}` `{level}` `{target}` `{message}`) → the custom
//!     [`TokenLayer`] (console + optional rotating file).
//!   - `None` → the default `fmt` layers (console + optional rotating file).
//! - The KUBERNETES profile default is threaded into [`init`] as `profile_format_default` (the
//!   resolved platform is known before the component config loads); precedence FR-RT-3.
//! - Limitation: the format and file layers are decided at [`init`] time only
//!   (tracing layers cannot be added/removed after install), so a `rust_format` or
//!   `fileLogging` change on hot reload does not take effect until restart; the
//!   *level* (root + per-logger `logging.loggers` overrides) reconfigures live.
//!
//! ## Usage Example
//! ```
//! use edgecommons::config::model::Config;
//! use edgecommons::logging;
//! use serde_json::json;
//!
//! let cfg = Config::from_value("c", "t", json!({ "logging": { "level": "DEBUG" } })).unwrap();
//! // `None` = no platform-profile default; pass the KUBERNETES profile's `Some("json")` to default
//! // to the stdout-JSON sink.
//! logging::init(&cfg, None);
//! ```
//!
//! ## Safety & Panics
//! None.
//!
//! ## Related Modules
//! - [`crate::config::model`], [`crate::config::ConfigurationChangeListener`],
//!   [`crate::config::template`].

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Map, Value};

use async_trait::async_trait;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, fmt, reload};

use crate::config::ConfigurationChangeListener;
use crate::config::model::Config;
use crate::config::template::resolve;

/// Type-erased "reload the level filter" callback, installed once by [`init`].
static RECONFIGURE: OnceLock<Box<dyn Fn(EnvFilter) + Send + Sync>> = OnceLock::new();

/// The `logging.rust_format` selector value (case-insensitive) that selects the structured
/// stdout-JSON sink (FR-LOG-4). The same `json` token selects the sink in every language.
const JSON_FORMAT: &str = "json";

/// Initialize the global tracing subscriber with a reloadable level filter and the configured
/// logging sink (FR-LOG-1/2/3/4).
///
/// `profile_format_default` is the resolved platform-profile's default logging format
/// ([`crate::platform::PlatformProfile::logging_format`], `Some("json")` on KUBERNETES). It is
/// threaded in by the builder because the resolved platform is known *before* the component config
/// loads. The effective format follows the FR-RT-3 precedence: explicit `logging.rust_format` ▸
/// this profile default ▸ `None` (the library default).
pub fn init(config: &Config, profile_format_default: Option<&str>) {
    let (filter_layer, handle) = reload::Layer::new(level_filter(config));
    let effective = effective_format(config, profile_format_default);

    // FR-LOG-2: the stdout-JSON sink is stdout-only — no rotating file appender is built, so a
    // read-only root FS is never touched. Off the json sink, the optional `logging.fileLogging`
    // rotating appender is unchanged. Building the writer at most once (and not at all under json).
    let file_writer = if installs_file_appender(config, effective.as_deref()) {
        file_make_writer(config)
    } else {
        None
    };

    // The effective format selects one of three sinks. Separate registry branches avoid boxing the
    // heterogeneous layer types; the level filter (and its reload handle) is shared by all three.
    let installed = match effective {
        // FR-LOG-1: structured stdout-JSON sink (one object per line). Best-effort correlation
        // fields are captured once here from the env + the resolved identity (FR-LOG-3).
        Some(ref fmt) if selects_json(fmt) => {
            let env: HashMap<String, String> = std::env::vars().collect();
            let correlation = correlation_fields(&env, &config.thing_name);
            tracing_subscriber::registry()
                .with(filter_layer)
                .with(JsonLayer { correlation })
                .try_init()
                .is_ok()
        }
        // A non-json token template → render every event through the token layer (console + file).
        Some(template) => tracing_subscriber::registry()
            .with(filter_layer)
            .with(TokenLayer {
                template,
                file: file_writer,
            })
            .try_init()
            .is_ok(),
        // Library default: the plain `fmt` console layer + optional rotating-file layer.
        None => {
            let file_layer =
                file_writer.map(|writer| fmt::layer().with_ansi(false).with_writer(writer));
            tracing_subscriber::registry()
                .with(filter_layer)
                .with(fmt::layer())
                .with(file_layer)
                .try_init()
                .is_ok()
        }
    };
    if installed {
        let _ = RECONFIGURE.set(Box::new(move |filter: EnvFilter| {
            let _ = handle.reload(filter);
        }));
    }
}

/// Resolve the *effective* logging format (FR-LOG-4, precedence FR-RT-3): explicit
/// `logging.rust_format` ▸ the platform-profile default (`json` on KUBERNETES) ▸ `None` (the
/// library console/text default). Pure — unit-testable without installing a subscriber.
fn effective_format(config: &Config, profile_format_default: Option<&str>) -> Option<String> {
    config
        .parsed
        .logging
        .rust_format
        .clone()
        .or_else(|| profile_format_default.map(str::to_string))
}

/// Whether `format` selects the structured stdout-JSON sink — the case-insensitive [`JSON_FORMAT`]
/// token (FR-LOG-4).
fn selects_json(format: &str) -> bool {
    format.eq_ignore_ascii_case(JSON_FORMAT)
}

/// Whether the resolved sink installs an in-process (rotating) file appender. The stdout-JSON sink
/// is **stdout-only** (FR-LOG-2): it never installs file rotation regardless of `logging.fileLogging`
/// (the cluster log agent owns rotation), so a read-only root FS cannot break logging on the
/// KUBERNETES default. Every other sink keeps the existing optional `logging.fileLogging` appender.
fn installs_file_appender(config: &Config, effective: Option<&str>) -> bool {
    if effective.is_some_and(selects_json) {
        return false;
    }
    config
        .parsed
        .logging
        .file_logging
        .as_ref()
        .is_some_and(|fl| fl.enabled)
}

/// A `tracing` layer that renders each event from a `rust_format` token template
/// (`{timestamp}` `{level}` `{target}` `{message}`) to stdout and, optionally, a
/// rotating file. Installed only when `logging.rust_format` is configured; the
/// format is fixed at [`init`] (tracing layers cannot be swapped at runtime).
struct TokenLayer {
    template: String,
    file: Option<RotatingFileMakeWriter>,
}

/// Render a `rust_format` token template. Unknown `{...}` tokens are left as-is.
fn render_template(
    template: &str,
    timestamp: &str,
    level: &str,
    target: &str,
    message: &str,
) -> String {
    template
        .replace("{timestamp}", timestamp)
        .replace("{level}", level)
        .replace("{target}", target)
        .replace("{message}", message)
}

/// Extracts the `message` field of an event into a string.
struct MessageVisitor<'a>(&'a mut String);

impl Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write as _;
            let _ = write!(self.0, "{value:?}");
        }
    }
}

impl<S> tracing_subscriber::Layer<S> for TokenLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));

        let mut timestamp = String::new();
        let _ = SystemTime.format_time(&mut fmt::format::Writer::new(&mut timestamp));

        let line = render_template(
            &self.template,
            timestamp.trim(),
            meta.level().as_str(),
            meta.target(),
            &message,
        );

        let _ = writeln!(io::stdout(), "{line}");
        if let Some(mw) = &self.file {
            let mut w = mw.make_writer();
            let _ = writeln!(w, "{line}");
        }
    }
}

/// A `tracing` layer that emits **one JSON object per line to stdout** (FR-LOG-1) — the default
/// sink on the KUBERNETES platform. Each line carries at least `timestamp`, `level`, `logger`
/// (the event target/name), and `message`, plus every other structured event field (so a logged
/// `error`/`exception` field is preserved verbatim — "thrown when present"), plus the best-effort
/// correlation fields captured at install (FR-LOG-3). The sink is **stdout-only**: no file rotation
/// is installed (FR-LOG-2). Installed only when the effective format is the `json` token; the
/// format is fixed at [`init`] (tracing layers cannot be swapped at runtime).
struct JsonLayer {
    /// `(json key, value)` for the present correlation fields (`pod`/`namespace`/`node`/`thing`).
    /// Absent fields are omitted entirely, so no empty/null noise is emitted.
    correlation: Vec<(&'static str, String)>,
}

/// Collects an event's fields into a JSON object. `message` and any user fields (e.g. an
/// `error`/`exception`) are captured under their own keys, each rendered to the closest JSON type.
struct JsonVisitor<'a>(&'a mut Map<String, Value>);

impl Visit for JsonVisitor<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().to_string(), Value::from(value));
    }
    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.0
            .insert(field.name().to_string(), Value::from(value.to_string()));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // The `message` field and any non-primitive field arrive here; `{:?}` of `format_args!`
        // renders the message text without extra quoting (matches the token-layer visitor).
        self.0
            .insert(field.name().to_string(), Value::from(format!("{value:?}")));
    }
}

/// Assemble a single JSON log line from the standard fields, the collected event `fields`, and the
/// correlation fields. The standard keys (`timestamp`/`level`/`logger`) and the correlation fields
/// take precedence over any same-named event field. `serde_json` guarantees valid, single-line
/// output (string escaping included), so embedded newlines in a message stay on one physical line.
fn build_json_line(
    timestamp: &str,
    level: &str,
    logger: &str,
    fields: Map<String, Value>,
    correlation: &[(&'static str, String)],
) -> Option<String> {
    // Start from the event fields so the authoritative keys below win on any collision.
    let mut map = fields;
    map.insert("timestamp".to_string(), Value::from(timestamp));
    map.insert("level".to_string(), Value::from(level));
    map.insert("logger".to_string(), Value::from(logger));
    for (key, value) in correlation {
        map.insert((*key).to_string(), Value::from(value.clone()));
    }
    serde_json::to_string(&Value::Object(map)).ok()
}

impl<S> tracing_subscriber::Layer<S> for JsonLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut fields = Map::new();
        event.record(&mut JsonVisitor(&mut fields));

        let mut timestamp = String::new();
        let _ = SystemTime.format_time(&mut fmt::format::Writer::new(&mut timestamp));

        if let Some(line) = build_json_line(
            timestamp.trim(),
            meta.level().as_str(),
            meta.target(),
            fields,
            &self.correlation,
        ) {
            let _ = writeln!(io::stdout(), "{line}");
        }
    }
}

/// Build the best-effort logging correlation fields (FR-LOG-3) from the environment and the
/// resolved identity. Includes `pod`/`namespace`/`node` only when the matching Downward-API env var
/// ([`crate::platform::ENV_K8S_POD_NAME`] / [`crate::platform::ENV_K8S_POD_NAMESPACE`] /
/// [`crate::platform::ENV_K8S_NODE_NAME`] — the same vars wired in Phase 1b) is present and
/// non-empty, and `thing` when the resolved identity is non-empty. Absent values are omitted (no
/// empty/null noise). Pure (the env is injected) so it is unit-testable.
fn correlation_fields(env: &HashMap<String, String>, thing: &str) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    for (key, var) in [
        ("pod", crate::platform::ENV_K8S_POD_NAME),
        ("namespace", crate::platform::ENV_K8S_POD_NAMESPACE),
        ("node", crate::platform::ENV_K8S_NODE_NAME),
    ] {
        if let Some(value) = env.get(var) {
            if !value.is_empty() {
                fields.push((key, value.clone()));
            }
        }
    }
    if !thing.is_empty() {
        fields.push(("thing", thing.to_string()));
    }
    fields
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
                eprintln!("edgecommons: failed to create log directory {parent:?}: {e}");
                return None;
            }
        }
    }

    match RotatingFileWriter::open(path.clone(), max_bytes, backup_count) {
        Ok(writer) => Some(RotatingFileMakeWriter(Arc::new(Mutex::new(writer)))),
        Err(e) => {
            eprintln!("edgecommons: failed to open log file {path:?}: {e}");
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
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
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

        self.file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
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
            "edgecommons_logtest_{}_{}",
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
    fn render_template_substitutes_tokens() {
        let line = render_template(
            "{timestamp} {level} {target}: {message}",
            "2026-01-01T00:00:00Z",
            "INFO",
            "edgecommons::x",
            "hello world",
        );
        assert_eq!(
            line,
            "2026-01-01T00:00:00Z INFO edgecommons::x: hello world"
        );
        // Unknown tokens are left as-is; a different layout is honored.
        assert_eq!(
            render_template("[{level}] {message} {nope}", "t", "WARN", "tgt", "m"),
            "[WARN] m {nope}"
        );
    }

    #[test]
    fn config_parses_rust_format_key() {
        let cfg = Config::from_value(
            "c",
            "t",
            serde_json::json!({ "logging": { "rust_format": "{level}|{message}" } }),
        )
        .unwrap();
        assert_eq!(
            cfg.parsed.logging.rust_format.as_deref(),
            Some("{level}|{message}")
        );
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
        assert!(
            !w.backup_path(3).exists(),
            "backup_count=2 must not keep .3"
        );

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
        assert!(
            !w.backup_path(1).exists(),
            "backup_count=0 keeps no backups"
        );
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
        assert!(
            file_make_writer(&cfg).is_some(),
            "writer should be built when enabled"
        );

        let cfg_off = Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({ "logging": { "fileLogging": { "enabled": false } } }),
        )
        .unwrap();
        assert!(
            file_make_writer(&cfg_off).is_none(),
            "writer should be absent when disabled"
        );
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
        assert!(
            dir.join("MyComp.log").exists(),
            "template-resolved log file should be created"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- FR-LOG: stdout-JSON sink selection + precedence ----------

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn cfg_with(logging: serde_json::Value) -> Config {
        Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({ "logging": logging }),
        )
        .unwrap()
    }

    #[test]
    fn json_token_is_recognized_case_insensitively() {
        // FR-LOG-4: the consistent selector value is `json`, matched case-insensitively.
        assert!(selects_json("json"));
        assert!(selects_json("JSON"));
        assert!(selects_json("Json"));
        assert!(!selects_json("console"));
        assert!(!selects_json("{level}|{message}"));
        assert!(!selects_json("jsonish"));
    }

    #[test]
    fn effective_format_precedence_explicit_then_profile_then_default() {
        // Explicit config wins over the profile default (FR-RT-3).
        let cfg = cfg_with(serde_json::json!({ "rust_format": "{level}|{message}" }));
        assert_eq!(
            Some("{level}|{message}".to_string()),
            effective_format(&cfg, Some("json"))
        );
        // No explicit config → the platform-profile default applies (json on KUBERNETES).
        let cfg = cfg_with(serde_json::json!({}));
        assert_eq!(
            Some("json".to_string()),
            effective_format(&cfg, Some("json"))
        );
        // No explicit config and no profile default (GREENGRASS/HOST) → library default (None).
        assert_eq!(None, effective_format(&cfg, None));
    }

    #[test]
    fn explicit_non_json_format_overrides_k8s_json_default() {
        // A pod that sets a non-json token must NOT get the json sink (FR-LOG-4 / FR-RT-3).
        let cfg = cfg_with(serde_json::json!({ "rust_format": "{message}" }));
        let effective = effective_format(&cfg, Some("json")).unwrap();
        assert!(!selects_json(&effective));
    }

    #[test]
    fn explicit_json_selects_sink_even_off_kubernetes() {
        // The `json` token selects the sink regardless of platform (no profile default needed).
        let cfg = cfg_with(serde_json::json!({ "rust_format": "json" }));
        let effective = effective_format(&cfg, None).unwrap();
        assert!(selects_json(&effective));
    }

    #[test]
    fn json_sink_installs_no_file_appender_even_with_file_logging_enabled() {
        // FR-LOG-2: under the json sink the rotating file appender is NOT installed — stdout-only,
        // so a read-only root FS cannot break logging — even if `fileLogging` is set in config.
        let cfg = cfg_with(serde_json::json!({
            "fileLogging": { "enabled": true, "filePath": "/nonexistent/should-not-open.log" }
        }));
        assert!(!installs_file_appender(&cfg, Some("json")));
        assert!(!installs_file_appender(&cfg, Some("JSON")));
    }

    #[test]
    fn non_json_sinks_keep_the_file_appender_contract() {
        // Off the json sink, `fileLogging.enabled` still drives the rotating appender.
        let on = cfg_with(serde_json::json!({
            "fileLogging": { "enabled": true, "filePath": "/var/log/app.log" }
        }));
        assert!(installs_file_appender(&on, None));
        assert!(installs_file_appender(&on, Some("{message}")));
        // Disabled / absent fileLogging → no appender, as today.
        let off = cfg_with(serde_json::json!({ "fileLogging": { "enabled": false } }));
        assert!(!installs_file_appender(&off, None));
        let none = cfg_with(serde_json::json!({}));
        assert!(!installs_file_appender(&none, None));
    }

    // ---------- FR-LOG-1: one-object-per-line JSON ----------

    #[test]
    fn build_json_line_emits_valid_single_object_with_required_fields() {
        let mut fields = Map::new();
        fields.insert("message".to_string(), Value::from("hello world"));
        let line = build_json_line(
            "2026-01-01T00:00:00Z",
            "INFO",
            "edgecommons::x",
            fields,
            &[],
        )
        .unwrap();

        // Exactly one line (no embedded raw newline).
        assert_eq!(1, line.lines().count(), "must be one JSON object per line");
        let v: Value = serde_json::from_str(&line).expect("each line must be valid JSON");
        assert_eq!("2026-01-01T00:00:00Z", v["timestamp"]);
        assert_eq!("INFO", v["level"]);
        assert_eq!("edgecommons::x", v["logger"]);
        assert_eq!("hello world", v["message"]);
    }

    #[test]
    fn build_json_line_includes_correlation_when_present_and_omits_when_absent() {
        // FR-LOG-3: correlation fields appear when present...
        let mut fields = Map::new();
        fields.insert("message".to_string(), Value::from("m"));
        let correlation = vec![
            ("pod", "pod-7".to_string()),
            ("namespace", "ns".to_string()),
            ("node", "node-a".to_string()),
            ("thing", "thing-1".to_string()),
        ];
        let line = build_json_line("t", "WARN", "tgt", fields, &correlation).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!("pod-7", v["pod"]);
        assert_eq!("ns", v["namespace"]);
        assert_eq!("node-a", v["node"]);
        assert_eq!("thing-1", v["thing"]);

        // ...and are entirely absent (not null/empty) when no correlation is supplied.
        let line = build_json_line("t", "WARN", "tgt", Map::new(), &[]).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("pod"));
        assert!(!obj.contains_key("namespace"));
        assert!(!obj.contains_key("node"));
        assert!(!obj.contains_key("thing"));
    }

    #[test]
    fn build_json_line_preserves_exception_field_and_stays_one_line() {
        // "thrown/exception when present": a structured error field is preserved; a message with an
        // embedded newline is escaped, not split across physical lines.
        let mut fields = Map::new();
        fields.insert("message".to_string(), Value::from("line1\nline2"));
        fields.insert(
            "exception".to_string(),
            Value::from("BrokenPipe: connection reset"),
        );
        let line = build_json_line("t", "ERROR", "tgt", fields, &[]).unwrap();
        assert_eq!(
            1,
            line.lines().count(),
            "embedded newline must stay one physical line"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!("line1\nline2", v["message"]);
        assert_eq!("BrokenPipe: connection reset", v["exception"]);
    }

    #[test]
    fn build_json_line_standard_keys_win_over_colliding_event_fields() {
        // A user field that collides with a reserved key must not shadow the authoritative value.
        let mut fields = Map::new();
        fields.insert("level".to_string(), Value::from("bogus"));
        fields.insert("message".to_string(), Value::from("m"));
        let line = build_json_line("t", "INFO", "tgt", fields, &[]).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            "INFO", v["level"],
            "the real level must win over a colliding field"
        );
    }

    // ---------- FR-LOG-3: correlation_fields from the Downward API ----------

    #[test]
    fn correlation_fields_from_full_downward_api_env() {
        let e = env(&[
            (crate::platform::ENV_K8S_POD_NAME, "pod-7"),
            (crate::platform::ENV_K8S_POD_NAMESPACE, "team-a"),
            (crate::platform::ENV_K8S_NODE_NAME, "node-1"),
        ]);
        let fields = correlation_fields(&e, "thing-1");
        assert_eq!(
            vec![
                ("pod", "pod-7".to_string()),
                ("namespace", "team-a".to_string()),
                ("node", "node-1".to_string()),
                ("thing", "thing-1".to_string()),
            ],
            fields
        );
    }

    #[test]
    fn correlation_fields_omits_absent_and_empty_env_values() {
        // Empty env values are not signals; missing vars are skipped. Only `thing` remains.
        let e = env(&[
            (crate::platform::ENV_K8S_POD_NAME, ""),
            (crate::platform::ENV_K8S_NODE_NAME, "node-1"),
        ]);
        let fields = correlation_fields(&e, "thing-1");
        assert_eq!(
            vec![
                ("node", "node-1".to_string()),
                ("thing", "thing-1".to_string())
            ],
            fields
        );
    }

    #[test]
    fn correlation_fields_omits_thing_when_identity_empty() {
        let fields = correlation_fields(&env(&[]), "");
        assert!(
            fields.is_empty(),
            "no env and empty identity → no correlation noise"
        );
    }

    // ---------- init / reconfigure (the subscriber install + the three sink branches) ----------

    #[test]
    fn init_installs_subscriber_and_reconfigure_reapplies_level() {
        // No other unit test installs a global subscriber, so this `init` wins the one-time install
        // and exercises the `installed` block + RECONFIGURE wiring. The two follow-up `init` calls
        // can no longer install (global already set) but still execute their sink-branch construction.
        let dir = test_dir("init");

        // Default sink (no rust_format) + file logging → the `None` branch builds the file appender.
        let cfg_default = Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({ "logging": { "level": "DEBUG",
                "fileLogging": { "enabled": true, "filePath": dir.join("a.log").to_string_lossy() } } }),
        )
        .unwrap();
        init(&cfg_default, None);

        // JSON sink branch (captures correlation fields from the env + identity).
        init(
            &cfg_with(serde_json::json!({ "rust_format": "json", "level": "INFO" })),
            None,
        );

        // Token sink branch + file appender.
        let cfg_token = Config::from_value(
            "com.example.C",
            "thing-1",
            serde_json::json!({ "logging": { "rust_format": "{level} {message}",
                "fileLogging": { "enabled": true, "filePath": dir.join("b.log").to_string_lossy() } } }),
        )
        .unwrap();
        init(&cfg_token, None);

        // reconfigure re-applies the level (incl. a per-logger override directive). Safe no-op when
        // the subscriber was never installed; here it is, so the reload handle swaps the filter.
        let cfg_reload =
            cfg_with(serde_json::json!({ "level": "WARN", "loggers": { "edgecommons": "debug" } }));
        reconfigure(&cfg_reload);

        // The hot-reload listener wrapper drives reconfigure on a config change.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(
                LoggingReconfigurer
                    .on_configuration_change(Arc::new(cfg_reload))
                    .await
            );
        });

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_filter_applies_root_and_per_logger_directives() {
        let cfg = cfg_with(serde_json::json!({
            "level": "warn",
            "loggers": { "edgecommons::messaging": "debug", "rumqttc": "error" }
        }));
        // Rendering the filter is enough to prove the directives parse; the EnvFilter Display lists
        // them. (A malformed per-logger entry would fall back to the bare root level.)
        let rendered = level_filter(&cfg).to_string();
        assert!(
            rendered.contains("edgecommons::messaging=debug"),
            "got {rendered}"
        );
        assert!(rendered.contains("rumqttc=error"), "got {rendered}");
    }

    // ---------- the token + json layers (driven via a thread-local subscriber) ----------

    #[test]
    fn token_layer_renders_event_to_stdout_and_file() {
        let dir = test_dir("tokenlayer");
        let base = dir.join("t.log");
        let mw = RotatingFileMakeWriter(Arc::new(Mutex::new(
            RotatingFileWriter::open(base.clone(), 0, 1).unwrap(),
        )));
        let layer = TokenLayer {
            template: "{level}|{target}|{message}".to_string(),
            file: Some(mw),
        };
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("hello token");
        });
        // The rendered line must have reached the rotating file via the MakeWriter + locked writer.
        let contents = read(&base);
        assert!(
            contents.contains("INFO|"),
            "level token rendered, got {contents:?}"
        );
        assert!(
            contents.contains("|hello token"),
            "message token rendered, got {contents:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_layer_emits_every_scalar_field_type_without_dropping_events() {
        // Drives JsonLayer::on_event and every JsonVisitor branch (str/bool/i64/u64/f64/error/debug).
        let layer = JsonLayer {
            correlation: vec![("thing", "t1".to_string())],
        };
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("trace"))
            .with(layer);
        let io_err = std::io::Error::other("boom");
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                s = "str",
                b = true,
                i = -3_i64,
                u = 7_u64,
                f = 1.5_f64,
                err = &io_err as &dyn std::error::Error,
                "json line"
            );
        });
        // No panic + the layer accepted the event for every field type (the JSON shape itself is
        // asserted by the build_json_line tests above).
    }

    #[test]
    fn file_make_writer_returns_none_when_the_file_cannot_be_opened() {
        // filePath points at an existing *directory*, so opening it as an append file fails — the
        // builder reports to stderr and yields None (file logging is skipped, never a panic).
        let dir = test_dir("openfail");
        let cfg = Config::from_value(
            "c",
            "t",
            serde_json::json!({
                "logging": { "fileLogging": { "enabled": true, "filePath": dir.to_string_lossy() } }
            }),
        )
        .unwrap();
        assert!(file_make_writer(&cfg).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
