//! # Configuration source — CONFIGMAP (Kubernetes-native)
//!
//! **One-liner purpose**: Load the component config from a mounted **ConfigMap directory** and
//! hot-reload it across the kubelet's atomic `..data` symlink swap (DESIGN-subsystems §1,
//! FR-CFG-1..5). The default config source on the `KUBERNETES` platform.
//!
//! ## Overview
//! Selected via `-c CONFIGMAP [mountDir] [key]`; defaults are mount dir `/etc/edgecommons` and key
//! `config.json` (so a pod with a ConfigMap mounted at `/etc/edgecommons` loads `config.json` with no
//! `-c` flag). It is the canonical analogue of [`super::file::FileConfigSource`] — same
//! load/validate/reload seam — but it watches the mount *directory* instead of the file inode.
//!
//! ## Why not [`super::file::FileConfigSource`]?
//! A mounted ConfigMap is a directory of symlinks the kubelet swaps atomically: the user-visible
//! `config.json` points at `..data/config.json`, and `..data` is itself a symlink the kubelet swaps
//! (write a new timestamped dir, stage `..data_tmp` → it, then `rename(..data_tmp, ..data)`).
//! Crucially:
//! - a watch on the user-visible *file* fires once and dies after the swap (the inode it pointed at
//!   is gone — `IN_DELETE_SELF`); and
//! - the swap manifests as events on the `..data`/`..data_tmp` entries, **not** on `config.json`, so
//!   a name-filtered watch never reloads.
//!
//! This source therefore (a) watches the mount directory, which persists across swaps; (b) reacts to
//! *any* entry event so the `..data` swap triggers a reload; and (c) **re-arms** — if the underlying
//! OS watch is lost (registration failed during a swap window, or the directory was replaced) it
//! re-registers after a short backoff rather than silently going dead (FR-CFG-2).
//!
//! ## Reject-and-keep (FR-CFG-5)
//! On a reload, a malformed read (a mid-swap window or a bad ConfigMap edit) must never crash a
//! running pod: the read is logged and skipped, so the previously-applied config stays in effect. A
//! parseable-but-schema-invalid document is rejected-and-kept downstream by the reload task
//! (`crate::spawn_config_reload`, which validates before publishing). The *initial* [`ConfigSource::load`]
//! still fails loudly, exactly like the FILE source.
//!
//! ## The `subPath` caveat (FR-CFG-3)
//! A ConfigMap mounted with `subPath` is **never** updated by the kubelet — there is no `..data`
//! symlink farm and hot-reload is silently dead. This source warns when it detects a mount with no
//! `..data` entry. Mount the whole volume (not a `subPath`); for a forced `subPath`/immutable/env
//! mount use a restart-on-change controller (e.g. Stakater Reloader).
//!
//! ## Projection artifacts (FR-CFG-4)
//! Kubelet projection artifacts (`..data`, `..2026_…` timestamped dirs, `..data_tmp`) are never
//! parsed as config: the configured key is rejected at construction if it is itself such an artifact,
//! using the shared dotfile filter [`is_projection_artifact`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use async_trait::async_trait;
use notify::{RecursiveMode, Watcher, recommended_watcher};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::ConfigSource;
use crate::error::{EdgeCommonsError, Result};

/// Default ConfigMap mount directory when `-c CONFIGMAP` is given no path argument.
pub const DEFAULT_MOUNT_DIR: &str = "/etc/edgecommons";
/// Default config key (file name within the mount) when none is given.
pub const DEFAULT_KEY: &str = "config.json";
/// The kubelet's atomic-swap symlink; its presence indicates a whole-volume (reloadable) mount.
const KUBELET_DATA_LINK: &str = "..data";
/// Backoff before re-registering after the directory watch is lost (re-arm).
const REARM_BACKOFF: Duration = Duration::from_millis(200);
/// Poll cadence for the event loop (so the stop flag is observed promptly).
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// True for kubelet/Docker volume-projection artifacts and hidden entries — any name beginning with
/// `'.'`. This is the single source of truth for the dotfile filter that skips the kubelet symlink
/// farm (`..data`, `..2026_…` timestamped dirs, the `..data_tmp` swap-staging entry). It mirrors the
/// canonical Java `MountedDirSource.isProjectionArtifact`; the parameters `MountedDirSource` reuses
/// it so the filter stays identical across the config and parameters subsystems (FR-CFG-4).
///
/// (The owner lives here, not in `parameters`, because the config source is always compiled while
/// the `parameters` module is feature-gated — so the dependency can only point this way.)
///
/// # Examples
/// ```
/// use edgecommons::config::source::configmap::is_projection_artifact;
/// assert!(is_projection_artifact("..data"));
/// assert!(is_projection_artifact("..2026_06_25_12_00_00.123456789"));
/// assert!(is_projection_artifact("..data_tmp"));
/// assert!(!is_projection_artifact("config.json"));
/// ```
pub fn is_projection_artifact(file_name: &str) -> bool {
    file_name.starts_with('.')
}

/// Loads configuration from a mounted ConfigMap directory, with directory-watch hot reload that
/// survives the kubelet `..data` symlink swap. See the module docs for the full rationale.
#[derive(Debug)]
pub struct ConfigMapConfigSource {
    mount_dir: PathBuf,
    key: String,
    config_file: PathBuf,
    /// Signals the background watch thread to stop (set on drop).
    stop: Arc<AtomicBool>,
    /// Retains the watch thread handle so it can be joined on drop.
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ConfigMapConfigSource {
    /// Create a ConfigMap config source.
    ///
    /// # Parameters
    /// - `mount_dir`: the ConfigMap mount directory, or `None` for [`DEFAULT_MOUNT_DIR`].
    /// - `key`: the config file name within the mount, or `None` for [`DEFAULT_KEY`].
    ///
    /// # Errors
    /// Returns [`EdgeCommonsError::Config`] if `key` is a kubelet projection artifact (a `..`/`.` entry),
    /// which must never be read as config (FR-CFG-4).
    pub fn new(mount_dir: Option<PathBuf>, key: Option<String>) -> Result<Self> {
        let mount_dir = mount_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_MOUNT_DIR));
        let key = key.unwrap_or_else(|| DEFAULT_KEY.to_string());
        if is_projection_artifact(&key) {
            return Err(EdgeCommonsError::Config(format!(
                "ConfigMap key must not be a kubelet projection artifact (a '..'/'.' entry): {key}"
            )));
        }
        let config_file = mount_dir.join(&key);
        warn_if_subpath_mount(&mount_dir);
        Ok(Self {
            mount_dir,
            key,
            config_file,
            stop: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        })
    }

    /// Read and parse the config file, returning `None` (reject-and-keep) on a transient/malformed
    /// read or an empty/`null` document, so a reload never crashes a running pod (FR-CFG-5).
    fn read_and_parse(config_file: &Path) -> Option<Value> {
        match std::fs::read(config_file) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) if v.is_null() => {
                    tracing::warn!("ConfigMap reload yielded empty config (keeping previous)");
                    None
                }
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(error = %e, "ignoring malformed ConfigMap reload (keeping previous)");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e, "ConfigMap reload read failed (keeping previous)");
                None
            }
        }
    }
}

/// Warn when the mount appears to be a `subPath` (or otherwise non-projected) mount that will never
/// hot-reload — detected by the absence of the kubelet `..data` symlink (FR-CFG-3).
fn warn_if_subpath_mount(mount_dir: &Path) {
    if !mount_dir.join(KUBELET_DATA_LINK).exists() {
        tracing::warn!(
            mount = %mount_dir.display(),
            "ConfigMap mount has no '..data' symlink — this looks like a subPath/immutable mount, \
             which the kubelet never updates, so hot-reload is disabled. Mount the whole volume \
             (not a subPath), or use a restart-on-change controller."
        );
    }
}

#[async_trait]
impl ConfigSource for ConfigMapConfigSource {
    async fn load(&self) -> Result<Value> {
        // Initial load fails loudly (parity with FILE), unlike a reload (reject-and-keep).
        let bytes = tokio::fs::read(&self.config_file).await.map_err(|e| {
            EdgeCommonsError::Config(format!(
                "Error reading ConfigMap configuration '{}': {e}",
                self.config_file.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|e| {
            EdgeCommonsError::Config(format!(
                "Error parsing ConfigMap configuration '{}': {e}",
                self.config_file.display()
            ))
        })
    }

    fn source_name(&self) -> &str {
        "CONFIGMAP"
    }

    fn watch(&self) -> Option<UnboundedReceiver<Value>> {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let dir = self.mount_dir.clone();
        let config_file = self.config_file.clone();
        let stop = self.stop.clone();

        let handle = std::thread::Builder::new()
            .name("edgecommons-configmap-watch".into())
            .spawn(move || watch_loop(dir, config_file, out_tx, stop))
            .ok()?;

        if let Ok(mut slot) = self.thread.lock() {
            *slot = Some(handle);
        }
        tracing::info!(
            mount = %self.mount_dir.display(),
            key = %self.key,
            "watching ConfigMap directory for changes"
        );
        Some(out_rx)
    }
}

impl Drop for ConfigMapConfigSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut slot) = self.thread.lock() {
            if let Some(handle) = slot.take() {
                let _ = handle.join();
            }
        }
    }
}

/// The re-arm outer loop: (re)register an OS watch on the mount directory and run the inner event
/// loop. If the watch is lost (registration failed during a swap window, or the directory was
/// replaced) drop out, back off, and re-register, so the watch survives inode replacement rather
/// than dying (FR-CFG-2). Returns when the source is stopped.
fn watch_loop(
    dir: PathBuf,
    config_file: PathBuf,
    out_tx: mpsc::UnboundedSender<Value>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::SeqCst) {
        // Channel-based watcher so the inner loop can detect watcher death and re-arm.
        let (ev_tx, ev_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match recommended_watcher(move |res| {
            let _ = ev_tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "ConfigMap watcher could not be created; retrying");
                backoff(&stop);
                continue;
            }
        };

        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            // The directory may not exist yet (e.g. a not-yet-mounted volume, or a swap window).
            tracing::warn!(error = %e, dir = %dir.display(), "ConfigMap watch could not arm; retrying");
            backoff(&stop);
            continue;
        }
        tracing::debug!(dir = %dir.display(), "ConfigMap directory watch armed");

        match inner_loop(&ev_rx, &config_file, &out_tx, &stop) {
            LoopExit::Stopped => return,
            LoopExit::ReceiverGone => return, // EdgeCommons dropped — stop quietly.
            LoopExit::Rearm => {
                tracing::warn!(dir = %dir.display(), "ConfigMap directory watch lost; re-arming");
                backoff(&stop);
            }
        }
    }
}

/// Why the inner event loop exited.
enum LoopExit {
    /// The source was stopped (drop) — exit without re-arming.
    Stopped,
    /// The config consumer dropped the receiver — exit without re-arming.
    ReceiverGone,
    /// The OS watch was lost — the outer loop should re-arm.
    Rearm,
}

/// The inner event loop for a single armed watcher: on *any* directory entry event (including the
/// `..data` swap) re-read the config and forward it, reject-and-keep on a malformed read. Polls with
/// a timeout so the stop flag is observed promptly.
fn inner_loop(
    ev_rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    config_file: &Path,
    out_tx: &mpsc::UnboundedSender<Value>,
    stop: &Arc<AtomicBool>,
) -> LoopExit {
    loop {
        if stop.load(Ordering::SeqCst) {
            return LoopExit::Stopped;
        }
        match ev_rx.recv_timeout(POLL_INTERVAL) {
            Ok(Ok(_event)) => {
                // Any entry change (including the ..data symlink swap) triggers a re-read.
                if let Some(value) = ConfigMapConfigSource::read_and_parse(config_file) {
                    if out_tx.send(value).is_err() {
                        return LoopExit::ReceiverGone;
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "ConfigMap watch error; re-arming");
                return LoopExit::Rearm;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return LoopExit::Rearm,
        }
    }
}

/// Sleep [`REARM_BACKOFF`], unless the source has been stopped (so a persistently-missing directory
/// does not spin the CPU and drop is still prompt).
fn backoff(stop: &Arc<AtomicBool>) {
    if !stop.load(Ordering::SeqCst) {
        std::thread::sleep(REARM_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn config_json(version: u32) -> String {
        format!("{{\"component\":{{\"name\":\"x\"}},\"version\":{version}}}")
    }

    fn write(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
    }

    /// Block until `pred` is true (polling the receiver) or the deadline elapses; returns the last
    /// value that satisfied `pred`, if any.
    fn recv_until<F>(
        rx: &mut UnboundedReceiver<Value>,
        timeout: Duration,
        mut pred: F,
    ) -> Option<Value>
    where
        F: FnMut(&Value) -> bool,
    {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(v) => {
                    if pred(&v) {
                        return Some(v);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(mpsc::error::TryRecvError::Disconnected) => return None,
            }
        }
        None
    }

    // ---------- load ----------

    #[tokio::test]
    async fn loads_config_from_mounted_directory() {
        let mount = tempfile::tempdir().unwrap();
        write(&mount.path().join("config.json"), &config_json(7));

        let source = ConfigMapConfigSource::new(
            Some(mount.path().to_path_buf()),
            Some("config.json".into()),
        )
        .unwrap();
        let loaded = source.load().await.unwrap();
        assert_eq!(loaded["version"], 7);
        assert_eq!(source.source_name(), "CONFIGMAP");
    }

    #[tokio::test]
    async fn load_fails_loudly_for_missing_key_on_initial_load() {
        // The initial load must fail loudly (parity with FILE), unlike a reload (reject-and-keep).
        let mount = tempfile::tempdir().unwrap();
        let source = ConfigMapConfigSource::new(
            Some(mount.path().to_path_buf()),
            Some("config.json".into()),
        )
        .unwrap();
        let err = source.load().await.unwrap_err();
        assert!(err.to_string().contains("config.json"));
    }

    #[test]
    fn applies_default_mount_dir_and_key_when_none() {
        // Defaults: /etc/edgecommons + config.json. The dir need not exist to construct.
        let source = ConfigMapConfigSource::new(None, None).unwrap();
        assert_eq!(
            source.config_file,
            Path::new(DEFAULT_MOUNT_DIR).join(DEFAULT_KEY)
        );
        assert_eq!(source.key, DEFAULT_KEY);
    }

    // ---------- dotfile filter (FR-CFG-4) ----------

    #[test]
    fn dotfile_filter_identifies_projection_artifacts() {
        assert!(is_projection_artifact("..data"));
        assert!(is_projection_artifact("..2026_06_25_12_00_00.123456789"));
        assert!(is_projection_artifact("..data_tmp"));
        assert!(!is_projection_artifact("config.json"));
    }

    #[test]
    fn rejects_key_that_is_a_projection_artifact() {
        let mount = tempfile::tempdir().unwrap();
        let err =
            ConfigMapConfigSource::new(Some(mount.path().to_path_buf()), Some("..data".into()))
                .unwrap_err();
        assert!(err.to_string().contains("projection artifact"));
    }

    // ---------- subPath warning (FR-CFG-3) ----------

    #[tokio::test]
    async fn constructs_when_subpath_mount_has_no_data_link() {
        // No '..data' symlink -> looks like a subPath mount; the source warns but still loads.
        let mount = tempfile::tempdir().unwrap();
        write(&mount.path().join("config.json"), &config_json(1));
        let source = ConfigMapConfigSource::new(
            Some(mount.path().to_path_buf()),
            Some("config.json".into()),
        )
        .unwrap();
        assert_eq!(source.load().await.unwrap()["version"], 1);
    }

    // ---------- reject-and-keep on reload (FR-CFG-5) ----------

    #[test]
    fn read_and_parse_returns_value_for_valid_json() {
        let mount = tempfile::tempdir().unwrap();
        let file = mount.path().join("config.json");
        write(&file, &config_json(3));
        let v = ConfigMapConfigSource::read_and_parse(&file).unwrap();
        assert_eq!(v["version"], 3);
    }

    #[test]
    fn read_and_parse_keeps_previous_on_malformed_json() {
        let mount = tempfile::tempdir().unwrap();
        let file = mount.path().join("config.json");
        write(&file, "{ this is : not valid json ]");
        assert!(ConfigMapConfigSource::read_and_parse(&file).is_none());
    }

    #[test]
    fn read_and_parse_keeps_previous_when_file_missing_mid_swap() {
        let mount = tempfile::tempdir().unwrap();
        let file = mount.path().join("config.json"); // never created
        assert!(ConfigMapConfigSource::read_and_parse(&file).is_none());
    }

    #[test]
    fn read_and_parse_keeps_previous_on_empty_file() {
        let mount = tempfile::tempdir().unwrap();
        let file = mount.path().join("config.json");
        write(&file, "");
        assert!(ConfigMapConfigSource::read_and_parse(&file).is_none());
    }

    // ---------- directory-watch re-arm across swaps (FR-CFG-2) ----------

    #[tokio::test(flavor = "multi_thread")]
    async fn directory_watch_reloads_repeatedly_across_edits() {
        // The watch must keep firing across successive in-place ConfigMap edits — i.e. it re-arms /
        // is not a one-shot watch. Cross-platform (in-place writes; the faithful symlink swap is the
        // unix-only test below).
        let mount = tempfile::tempdir().unwrap();
        let file = mount.path().join("config.json");
        write(&file, &config_json(1));
        std::fs::create_dir(mount.path().join("..data")).unwrap(); // whole-volume look: no subPath warn

        let source = ConfigMapConfigSource::new(
            Some(mount.path().to_path_buf()),
            Some("config.json".into()),
        )
        .unwrap();
        let mut rx = source.watch().unwrap();
        std::thread::sleep(Duration::from_millis(800)); // let the watch arm

        write(&file, &config_json(2));
        std::thread::sleep(Duration::from_millis(400));
        write(&file, &config_json(3));

        // The watch survived the first edit and re-fired on the second; the final value is v3.
        let got = recv_until(&mut rx, Duration::from_secs(10), |v| v["version"] == 3);
        assert_eq!(got.expect("expected a reload to version 3")["version"], 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn re_arms_when_directory_appears_later() {
        // Watch a directory that does not exist yet: registration fails, the watcher backs off and
        // retries (re-arm). Once the directory and the key appear, it delivers a reload.
        let parent = tempfile::tempdir().unwrap();
        let mount = parent.path().join("late-mount");

        let source =
            ConfigMapConfigSource::new(Some(mount.clone()), Some("config.json".into())).unwrap();
        let mut rx = source.watch().unwrap();

        std::thread::sleep(Duration::from_millis(300)); // a few register-retry cycles
        std::fs::create_dir(&mount).unwrap();
        std::thread::sleep(Duration::from_millis(800)); // let the watch arm on the new dir
        write(&mount.join("config.json"), &config_json(5));

        let got = recv_until(&mut rx, Duration::from_secs(10), |v| v["version"] == 5);
        assert_eq!(
            got.expect("watcher should re-arm and fire once the dir appears")["version"],
            5
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn hot_reload_survives_kubelet_data_symlink_swap() {
        // The faithful kubelet shape: config.json -> ..data/config.json, and ..data is a symlink the
        // kubelet swaps atomically (new timestamped dir, stage ..data_tmp, rename onto ..data).
        use std::os::unix::fs::symlink;

        let mount = tempfile::tempdir().unwrap();
        let m = mount.path();

        let first = m.join("..2026_a");
        std::fs::create_dir(&first).unwrap();
        write(&first.join("config.json"), &config_json(1));
        symlink("..2026_a", m.join("..data")).unwrap();
        symlink("..data/config.json", m.join("config.json")).unwrap();

        let source =
            ConfigMapConfigSource::new(Some(m.to_path_buf()), Some("config.json".into())).unwrap();
        assert_eq!(source.load().await.unwrap()["version"], 1);
        let mut rx = source.watch().unwrap();
        std::thread::sleep(Duration::from_millis(800)); // let the watch arm before the swap

        // Kubelet swap: new timestamped dir, stage ..data_tmp -> it, atomic rename onto ..data.
        let second = m.join("..2026_b");
        std::fs::create_dir(&second).unwrap();
        write(&second.join("config.json"), &config_json(2));
        symlink("..2026_b", m.join("..data_tmp")).unwrap();
        std::fs::rename(m.join("..data_tmp"), m.join("..data")).unwrap();

        let got = recv_until(&mut rx, Duration::from_secs(10), |v| v["version"] == 2);
        assert_eq!(
            got.expect("reload should survive the ..data swap")["version"],
            2
        );
    }
}
