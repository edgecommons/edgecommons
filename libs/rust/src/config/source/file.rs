//! # Configuration source — FILE
//!
//! **One-liner purpose**: Load configuration from a JSON file on disk, with
//! file-watch hot reload.
//!
//! ## Overview
//! Reads and parses the configured path on [`ConfigSource::load`]. [`ConfigSource::watch`]
//! installs an OS file watcher (via `notify`) on the file's directory and emits a
//! fresh config document whenever the file is created or modified.
//!
//! ## Semantics & Architecture
//! - Async file read (`tokio::fs`); the watcher callback reads synchronously on the
//!   `notify` thread and forwards parsed documents over an unbounded channel.
//! - The watcher is retained inside the source, so the source must outlive the
//!   receiver returned by `watch` (the runtime keeps it alive).
//! - Malformed reloads are logged and skipped (the previous config stays in effect).
//! - Error handling: [`crate::error::EdgeCommonsError::Io`] / [`crate::error::EdgeCommonsError::Json`].
//!
//! ## Usage Example
//! ```no_run
//! use edgecommons::config::source::{file::FileConfigSource, ConfigSource};
//! use std::path::PathBuf;
//! # async fn demo() -> edgecommons::Result<()> {
//! let source = FileConfigSource::new(PathBuf::from("config.json"));
//! let _doc = source.load().await?;
//! let mut updates = source.watch().expect("file source supports watching");
//! // hold `source` alive; receive new docs:
//! if let Some(_new_doc) = updates.recv().await { /* ... */ }
//! # Ok(())
//! # }
//! ```
//!
//! ## Related Modules
//! - [`super`], [`crate::config::validation`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use notify::{recommended_watcher, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver};

use super::ConfigSource;
use crate::error::Result;

/// Loads configuration from a JSON file on disk, with file-watch hot reload.
pub struct FileConfigSource {
    path: PathBuf,
    /// Retains the OS watcher so it keeps firing for the source's lifetime.
    watcher: Mutex<Option<RecommendedWatcher>>,
}

impl FileConfigSource {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            watcher: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ConfigSource for FileConfigSource {
    async fn load(&self) -> Result<Value> {
        let bytes = tokio::fs::read(&self.path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn source_name(&self) -> &str {
        "FILE"
    }

    fn watch(&self) -> Option<UnboundedReceiver<Value>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let target = self.path.clone();
        // Watch the parent directory so atomic rename-replace edits are caught.
        let dir = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let cb_target = target.clone();
        let mut watcher = match recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                return;
            }
            // Match by file name within the watched directory.
            let touched = event
                .paths
                .iter()
                .any(|p| p.file_name() == cb_target.file_name());
            if !touched {
                return;
            }
            match std::fs::read(&cb_target) {
                Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                    Ok(value) => {
                        let _ = tx.send(value);
                    }
                    Err(e) => tracing::warn!(error = %e, "ignoring malformed config file change"),
                },
                Err(e) => tracing::warn!(error = %e, "failed to read changed config file"),
            }
        }) {
            Ok(watcher) => watcher,
            Err(e) => {
                tracing::error!(error = %e, "failed to create config file watcher");
                return None;
            }
        };

        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            tracing::error!(error = %e, dir = %dir.display(), "failed to watch config directory");
            return None;
        }

        if let Ok(mut slot) = self.watcher.lock() {
            *slot = Some(watcher);
        }
        tracing::info!(path = %target.display(), "watching config file for changes");
        Some(rx)
    }
}
