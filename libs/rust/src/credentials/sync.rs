//! # Sync engine
//!
//! **One-liner purpose**: Seed and refresh the local vault from a [`CentralVaultSource`] —
//! offline-first, selective, rotation-aware.
//!
//! ## Semantics & Architecture
//! - **Bootstrap** (synchronous, at open): pull each configured secret so it's available
//!   immediately. **Refresh**: a background thread re-pulls on `refreshIntervalSecs`; `sync_now`
//!   forces a pass. Only changed secrets (different upstream version id) are written, as a new
//!   local version — the previous value is retained per the vault's `keep_versions` (rotation
//!   grace). **Offline-first**: a fetch failure logs and keeps the cached value (never clears it).
//! - The background thread stops (and is joined) on drop (RAII).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::central::CentralVaultSource;
use super::vault::{LocalVault, PutOptions};

/// Owns the background refresh thread; aborts + joins it on drop.
pub struct SyncEngine {
    inner: Arc<SyncInner>,
    handle: Option<JoinHandle<()>>,
}

struct SyncInner {
    vault: Arc<Mutex<LocalVault>>,
    source: Arc<dyn CentralVaultSource>,
    /// `(caller_name, central_id_override)`. The local key is the caller name under the namespace;
    /// the central id defaults to that same namespaced path (per-device) unless overridden (a
    /// shared/fleet secret).
    secrets: Vec<(String, Option<String>)>,
    namespace: String,
    stop: AtomicBool,
}

impl SyncEngine {
    /// Start syncing `secrets` from `source` into `vault` under `namespace`. Runs an immediate
    /// bootstrap pass when `bootstrap` is set, then refreshes every `interval_secs` (0 disables the
    /// background thread).
    pub fn start(
        vault: Arc<Mutex<LocalVault>>,
        source: Arc<dyn CentralVaultSource>,
        namespace: String,
        secrets: Vec<(String, Option<String>)>,
        interval_secs: u64,
        bootstrap: bool,
    ) -> Self {
        let inner = Arc::new(SyncInner { vault, source, secrets, namespace, stop: AtomicBool::new(false) });
        if bootstrap {
            inner.sync_once();
        }
        let handle = if interval_secs > 0 {
            let inner = inner.clone();
            Some(std::thread::spawn(move || {
                while !inner.stop.load(Ordering::Relaxed) {
                    // Sleep in 1s steps so stop is honored promptly.
                    for _ in 0..interval_secs {
                        if inner.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    if inner.stop.load(Ordering::Relaxed) {
                        return;
                    }
                    inner.sync_once();
                }
            }))
        } else {
            None
        };
        Self { inner, handle }
    }

    /// Force an immediate sync pass (the `refresh()` entry point).
    pub fn sync_now(&self) {
        self.inner.sync_once();
    }
}

impl SyncInner {
    /// The namespaced local key for a caller-facing name.
    fn local_key(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", self.namespace, name)
        }
    }

    fn sync_once(&self) {
        for (name, from) in &self.secrets {
            let local_key = self.local_key(name);
            // Central id defaults to the namespaced path (per-device); `from` overrides it to a
            // shared/fleet secret id.
            let central_id = from.clone().unwrap_or_else(|| local_key.clone());
            match self.source.fetch(&central_id) {
                Ok(Some(cs)) => {
                    let mut v = match self.vault.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    let _ = v.reload_if_changed();
                    // Skip if the latest local version already reflects this upstream version.
                    if v.latest_central_version_id(&local_key).as_deref() == Some(cs.central_version_id.as_str()) {
                        continue;
                    }
                    let opts = PutOptions {
                        source: Some("central".to_string()),
                        central_version_id: Some(cs.central_version_id),
                        labels: cs.labels,
                        ..PutOptions::default()
                    };
                    if let Err(e) = v.put(&local_key, &cs.bytes, opts) {
                        tracing::warn!(secret = %local_key, error = %e, "failed to write synced secret");
                    } else {
                        tracing::info!(secret = %local_key, central_id = %central_id, "secret synced from central");
                    }
                }
                Ok(None) => {
                    tracing::debug!(central_id = %central_id, "not present in central source; keeping local");
                }
                Err(e) => {
                    // Offline-first: keep the cached value, surface the staleness.
                    tracing::warn!(central_id = %central_id, error = %e, "central fetch failed; using cached value");
                }
            }
        }
    }
}

impl Drop for SyncEngine {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
