//! # Parameter service
//!
//! **One-liner purpose**: `gg.parameters()` returns a [`ParameterService`] — offline-first,
//! source-agnostic reads of externalized parameters. [`DefaultParameterService`] caches whatever a
//! [`ParameterSource`] provides and serves reads from the cache (never the network), refreshing the
//! declared names/paths selectively in the background / on demand.
//!
//! The cache is **source-aware**: a remote source (SSM, …) uses a persistent **encrypted** cache —
//! reusing the credentials [`LocalVault`] (same normative on-disk format) — so values survive
//! restarts and offline. An already-local source (`mountedDir`, `env`) uses an in-memory cache (the
//! backend is itself local + always available; re-persisting it would be redundant/regressive).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use super::source::ParameterSource;
use crate::credentials::{LocalVault, PutOptions};
use crate::error::EdgeCommonsError;
use crate::Result;

const SECURE_LABEL: &str = "secure";
const VERSION_LABEL: &str = "pversion";

/// A cached parameter value (decrypted, in memory). `secure` values must not be logged.
#[derive(Clone)]
struct Cached {
    value: Vec<u8>,
    secure: bool,
    version: Option<String>,
}

/// The cache layer behind the service (offline-first read store).
trait ParamCache: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<Cached>>;
    fn put(&self, name: &str, c: &Cached) -> Result<()>;
    fn entries(&self, prefix: &str) -> Result<Vec<(String, Cached)>>;
    fn len(&self) -> usize;
}

/// In-memory cache for already-local sources (`mountedDir`, `env`).
struct MemoryCache {
    map: RwLock<BTreeMap<String, Cached>>,
}

impl MemoryCache {
    fn new() -> Self {
        Self { map: RwLock::new(BTreeMap::new()) }
    }
    fn guard_read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<String, Cached>> {
        self.map.read().unwrap_or_else(|p| p.into_inner())
    }
}

impl ParamCache for MemoryCache {
    fn get(&self, name: &str) -> Result<Option<Cached>> {
        Ok(self.guard_read().get(name).cloned())
    }
    fn put(&self, name: &str, c: &Cached) -> Result<()> {
        self.map.write().unwrap_or_else(|p| p.into_inner()).insert(name.to_string(), c.clone());
        Ok(())
    }
    fn entries(&self, prefix: &str) -> Result<Vec<(String, Cached)>> {
        Ok(self
            .guard_read()
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
    fn len(&self) -> usize {
        self.guard_read().len()
    }
}

/// Persistent encrypted cache for remote sources — reuses the credentials [`LocalVault`] (the same
/// normative, cross-language on-disk format). The parameter value is the secret bytes; `secure` and
/// the upstream version ride along as labels.
struct VaultCache {
    vault: Arc<Mutex<LocalVault>>,
}

impl VaultCache {
    fn locked(&self) -> std::sync::MutexGuard<'_, LocalVault> {
        self.vault.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl ParamCache for VaultCache {
    fn get(&self, name: &str) -> Result<Option<Cached>> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        Ok(v.get(name)?.map(|s| Cached {
            value: s.bytes().to_vec(),
            secure: s.labels.get(SECURE_LABEL).map(|x| x == "true").unwrap_or(false),
            version: s.labels.get(VERSION_LABEL).cloned(),
        }))
    }
    fn put(&self, name: &str, c: &Cached) -> Result<()> {
        let mut labels = BTreeMap::new();
        labels.insert(SECURE_LABEL.to_string(), c.secure.to_string());
        if let Some(ver) = &c.version {
            labels.insert(VERSION_LABEL.to_string(), ver.clone());
        }
        let opts = PutOptions { source: Some("parameter".to_string()), labels, ..PutOptions::default() };
        let mut v = self.locked();
        v.reload_if_changed()?;
        v.put(name, &c.value, opts)?;
        Ok(())
    }
    fn entries(&self, prefix: &str) -> Result<Vec<(String, Cached)>> {
        let mut v = self.locked();
        v.reload_if_changed()?;
        let names: Vec<String> = v.list(prefix).into_iter().map(|m| m.name).collect();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            if let Some(s) = v.get(&name)? {
                out.push((
                    name,
                    Cached {
                        value: s.bytes().to_vec(),
                        secure: s.labels.get(SECURE_LABEL).map(|x| x == "true").unwrap_or(false),
                        version: s.labels.get(VERSION_LABEL).cloned(),
                    },
                ));
            }
        }
        Ok(out)
    }
    fn len(&self) -> usize {
        let mut v = self.locked();
        let _ = v.reload_if_changed();
        v.list("").len()
    }
}

/// Non-sensitive parameter-subsystem stats.
#[derive(Debug, Clone, Default)]
pub struct ParameterStats {
    pub parameter_count: u64,
    /// Age of the last successful refresh, ms (None if never refreshed).
    pub last_refresh_age_ms: Option<u64>,
    pub refresh_failures: u64,
    pub source: String,
}

/// The public parameter interface (depend on this, not [`DefaultParameterService`]).
pub trait ParameterService: Send + Sync {
    /// The value of `name` as a UTF-8 string, or `None`. Served from the local cache (offline-first).
    fn get(&self, name: &str) -> Result<Option<String>>;
    /// The raw value bytes of `name`.
    fn get_bytes(&self, name: &str) -> Result<Option<Vec<u8>>>;
    /// All cached parameters under `path` (the prefix), as name -> string value.
    fn get_by_path(&self, path: &str) -> Result<BTreeMap<String, String>>;
    /// Cached parameter names under `prefix` (metadata only — no values).
    fn names(&self, prefix: &str) -> Result<Vec<String>>;
    /// Force an immediate pull of the declared names/paths from the source into the cache.
    fn refresh(&self) -> Result<()>;
    /// Non-sensitive stats for observability.
    fn stats(&self) -> ParameterStats;

    /// The value parsed as an integer.
    fn get_int(&self, name: &str) -> Result<Option<i64>> {
        match self.get(name)? {
            Some(s) => s
                .trim()
                .parse::<i64>()
                .map(Some)
                .map_err(|e| EdgeCommonsError::Parameters(format!("parameter '{name}' is not an integer: {e}"))),
            None => Ok(None),
        }
    }
    /// The value parsed as a boolean (`true`/`false`/`1`/`0`, case-insensitive).
    fn get_bool(&self, name: &str) -> Result<Option<bool>> {
        match self.get(name)? {
            Some(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Some(true)),
                "false" | "0" | "no" | "off" => Ok(Some(false)),
                other => Err(EdgeCommonsError::Parameters(format!("parameter '{name}' is not a boolean: {other}"))),
            },
            None => Ok(None),
        }
    }
    /// The value parsed as JSON.
    fn get_json(&self, name: &str) -> Result<Option<serde_json::Value>> {
        match self.get_bytes(name)? {
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| EdgeCommonsError::Parameters(format!("parameter '{name}' is not JSON: {e}"))),
            None => Ok(None),
        }
    }
    /// A `StringList` value (comma-separated) as a list.
    fn get_string_list(&self, name: &str) -> Result<Option<Vec<String>>> {
        Ok(self.get(name)?.map(|s| {
            if s.is_empty() {
                Vec::new()
            } else {
                s.split(',').map(|x| x.trim().to_string()).collect()
            }
        }))
    }
}

/// The shared refresh-able core (source + cache + selection + counters). Held behind an `Arc` so the
/// background refresh thread and the service operate on the same state.
struct Inner {
    source: Arc<dyn ParameterSource>,
    cache: Arc<dyn ParamCache>,
    sync_names: Vec<String>,
    /// (path, recursive)
    sync_paths: Vec<(String, bool)>,
    last_refresh_ms: Mutex<Option<u64>>,
    failures: Mutex<u64>,
}

impl Inner {
    fn refresh(&self) -> Result<()> {
        let mut any_err: Option<EdgeCommonsError> = None;
        for name in &self.sync_names {
            match self.source.fetch(name) {
                Ok(Some(v)) => {
                    let _ = self.cache.put(name, &Cached { value: v.value, secure: v.secure, version: v.version });
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(parameter = %name, error = %e, "parameter refresh failed (keeping cached value)");
                    any_err = Some(e);
                }
            }
        }
        for (path, recursive) in &self.sync_paths {
            match self.source.fetch_by_path(path, *recursive) {
                Ok(items) => {
                    for (name, v) in items {
                        let _ = self.cache.put(&name, &Cached { value: v.value, secure: v.secure, version: v.version });
                    }
                }
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "parameter path refresh failed (keeping cached values)");
                    any_err = Some(e);
                }
            }
        }
        if let Some(e) = any_err {
            *self.failures.lock().unwrap_or_else(|p| p.into_inner()) += 1;
            // Offline-first: a refresh failure is non-fatal when we already have cached values.
            if self.cache.len() == 0 {
                return Err(e);
            }
        } else {
            *self.last_refresh_ms.lock().unwrap_or_else(|p| p.into_inner()) = Some(now_ms());
        }
        Ok(())
    }
}

/// Owns the background refresh thread; stops + joins it on drop (RAII). Network sources do blocking
/// I/O on their own runtime, so the periodic refresh runs on a dedicated OS thread (not a tokio
/// task) to avoid nesting runtimes.
struct Refresher {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Refresher {
    fn start(inner: Arc<Inner>, interval_secs: u64) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                use std::sync::atomic::Ordering;
                while !stop.load(Ordering::Relaxed) {
                    // Sleep in 1s steps so stop is honored promptly.
                    for _ in 0..interval_secs {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let _ = inner.refresh();
                }
            })
        };
        Self { stop, handle: Some(handle) }
    }
}

impl Drop for Refresher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Default [`ParameterService`]: a [`ParameterSource`] behind an offline-first cache, optionally
/// refreshed by a background thread.
pub struct DefaultParameterService {
    inner: Arc<Inner>,
    _refresher: Option<Refresher>,
}

impl DefaultParameterService {
    fn new(
        source: Arc<dyn ParameterSource>,
        cache: Arc<dyn ParamCache>,
        sync_names: Vec<String>,
        sync_paths: Vec<(String, bool)>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                source,
                cache,
                sync_names,
                sync_paths,
                last_refresh_ms: Mutex::new(None),
                failures: Mutex::new(0),
            }),
            _refresher: None,
        }
    }

    /// Build with a persistent encrypted cache (the credentials [`LocalVault`]) — for remote sources.
    pub fn with_persistent_cache(
        source: Arc<dyn ParameterSource>,
        vault: Arc<Mutex<LocalVault>>,
        sync_names: Vec<String>,
        sync_paths: Vec<(String, bool)>,
    ) -> Self {
        Self::new(source, Arc::new(VaultCache { vault }), sync_names, sync_paths)
    }

    /// Build with an in-memory cache — for already-local sources (`mountedDir`, `env`).
    pub fn with_memory_cache(
        source: Arc<dyn ParameterSource>,
        sync_names: Vec<String>,
        sync_paths: Vec<(String, bool)>,
    ) -> Self {
        Self::new(source, Arc::new(MemoryCache::new()), sync_names, sync_paths)
    }

    /// Start a background refresh thread that re-pulls the declared names/paths every
    /// `interval_secs` (0 disables it). The thread stops when the service is dropped (RAII).
    pub fn with_refresh(mut self, interval_secs: u64) -> Self {
        if interval_secs > 0 {
            self._refresher = Some(Refresher::start(self.inner.clone(), interval_secs));
        }
        self
    }
}

impl ParameterService for DefaultParameterService {
    fn get(&self, name: &str) -> Result<Option<String>> {
        match self.get_bytes(name)? {
            Some(b) => Ok(Some(
                String::from_utf8(b).map_err(|_| EdgeCommonsError::Parameters(format!("parameter '{name}' is not UTF-8")))?,
            )),
            None => Ok(None),
        }
    }
    fn get_bytes(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.inner.cache.get(name)?.map(|c| c.value))
    }
    fn get_by_path(&self, path: &str) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for (name, c) in self.inner.cache.entries(path)? {
            if let Ok(s) = String::from_utf8(c.value) {
                out.insert(name, s);
            }
        }
        Ok(out)
    }
    fn names(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self.inner.cache.entries(prefix)?.into_iter().map(|(n, _)| n).collect())
    }
    fn refresh(&self) -> Result<()> {
        self.inner.refresh()
    }
    fn stats(&self) -> ParameterStats {
        let last = *self.inner.last_refresh_ms.lock().unwrap_or_else(|p| p.into_inner());
        ParameterStats {
            parameter_count: self.inner.cache.len() as u64,
            last_refresh_age_ms: last.map(|ms| now_ms().saturating_sub(ms)),
            refresh_failures: *self.inner.failures.lock().unwrap_or_else(|p| p.into_inner()),
            source: self.inner.source.source_id().to_string(),
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
