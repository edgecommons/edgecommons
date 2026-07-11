//! # The destination: what a *sink* delivers to
//!
//! A sink consumes work and hands it to somewhere outside EdgeCommons — a filesystem, an object
//! store, an HTTP endpoint, a database. [`Destination`] is the seam. Implement it once per
//! backend; everything above it (retry, verification, reporting) is written against the trait and
//! never learns what a bucket is.
//!
//! ## The contract, and why each clause is there
//!
//! * **`deliver` is the commit.** When it returns `Ok`, the item is live at its final, *stable*
//!   key. Not staged, not pending — live.
//! * **The key is deterministic.** The same item always lands at the same place, so a redelivery
//!   is an **idempotent overwrite** rather than a duplicate. This is what makes retry safe: a
//!   sink that cannot retry without duplicating cannot retry at all.
//! * **`verify` runs before the source is released.** The whole point of a sink is that it is the
//!   last thing standing between data and its destination. Deleting the source because `deliver`
//!   returned `Ok` — without checking that what landed is what you sent — is how you lose the only
//!   copy.

use std::path::PathBuf;

use anyhow::Context;
use async_trait::async_trait;
use serde::Deserialize;

/// One unit of work to deliver: an opaque payload plus the stable key it belongs at.
#[derive(Debug, Clone)]
pub struct Item {
    /// The stable, deterministic key. Redelivering the same item overwrites in place.
    pub key: String,
    pub bytes: Vec<u8>,
}

/// Proof of what landed, returned by [`Destination::deliver`] and checked by
/// [`Destination::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub bytes_written: usize,
}

/// Why a delivery failed — and, crucially, **whether retrying could ever help**.
///
/// Getting this wrong is expensive in both directions: retrying a permanent failure burns the
/// budget and floods the log; giving up on a transient one loses data that a second attempt would
/// have delivered.
#[derive(Debug, thiserror::Error)]
// The local destination only ever fails transiently. A remote one -- bad credentials, a missing
// bucket, a malformed key -- is where `Permanent` earns its keep.
#[allow(dead_code)]
pub enum DeliverError {
    /// The world may differ next time: a timeout, a 503, a full disk that someone will empty.
    #[error("transient: {0}")]
    Transient(#[source] anyhow::Error),
    /// It will fail identically forever: bad credentials, a malformed key, a missing bucket.
    #[error("permanent: {0}")]
    Permanent(#[source] anyhow::Error),
}

impl DeliverError {
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}

pub type Result<T> = std::result::Result<T, DeliverError>;

/// A place a sink delivers to. **This is the trait you implement.**
#[async_trait]
pub trait Destination: Send + Sync {
    /// Its kind, as named in config (`local`, `s3`, …).
    fn kind(&self) -> &'static str;

    /// Deliver the item to its stable key. Returning `Ok` means it is **live**, not staged.
    async fn deliver(&self, item: &Item) -> Result<Delivered>;

    /// Confirm that what landed is what was sent — **before** the source is released.
    async fn verify(&self, item: &Item, delivered: &Delivered) -> Result<()>;
}

pub type SharedDestination = std::sync::Arc<dyn Destination>;

/// The destinations this component understands. Add a variant as you add a backend.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DestinationConfig {
    /// A directory on this device.
    Local { path: PathBuf },
}

/// Build a destination from config.
///
/// # Errors
///
/// If the configured destination cannot be constructed.
pub fn build(cfg: &DestinationConfig) -> anyhow::Result<SharedDestination> {
    match cfg {
        DestinationConfig::Local { path } => {
            Ok(std::sync::Arc::new(LocalDestination { root: path.clone() }))
        }
    }
}

/// A local-filesystem destination.
///
/// Small, but it demonstrates the two things every destination must get right: **write to a temp
/// file and rename** (a rename is atomic, so a reader never observes a half-written object, and a
/// crash mid-write leaves no corrupt artifact at the real key), and **derive the key
/// deterministically** so a redelivery overwrites rather than duplicates.
pub struct LocalDestination {
    pub root: PathBuf,
}

#[async_trait]
impl Destination for LocalDestination {
    fn kind(&self) -> &'static str {
        "local"
    }

    async fn deliver(&self, item: &Item) -> Result<Delivered> {
        let final_path = self.root.join(&item.key);
        let parent = final_path
            .parent()
            .map(std::borrow::ToOwned::to_owned)
            .unwrap_or_else(|| self.root.clone());

        tokio::fs::create_dir_all(&parent)
            .await
            .context("creating the destination directory")
            // A directory we cannot create is usually a permission or a path problem, and those
            // do not fix themselves — but a full disk does. Transient is the safer default:
            // a wrongly-transient failure wastes retries, a wrongly-permanent one loses data.
            .map_err(DeliverError::Transient)?;

        let tmp = parent.join(format!(".{}.partial", sanitize(&item.key)));
        tokio::fs::write(&tmp, &item.bytes)
            .await
            .context("writing the temp file")
            .map_err(DeliverError::Transient)?;

        // The atomic step. Until this returns, nothing exists at the real key.
        tokio::fs::rename(&tmp, &final_path)
            .await
            .context("renaming into place")
            .map_err(DeliverError::Transient)?;

        Ok(Delivered { bytes_written: item.bytes.len() })
    }

    async fn verify(&self, item: &Item, delivered: &Delivered) -> Result<()> {
        let path = self.root.join(&item.key);
        let meta = tokio::fs::metadata(&path)
            .await
            .context("stat-ing the delivered object")
            .map_err(DeliverError::Transient)?;

        let landed = usize::try_from(meta.len()).unwrap_or(usize::MAX);
        if landed != delivered.bytes_written {
            // The object is there but wrong. Do NOT release the source.
            return Err(DeliverError::Transient(anyhow::anyhow!(
                "size mismatch: wrote {} bytes, found {landed}",
                delivered.bytes_written
            )));
        }
        Ok(())
    }
}

/// Keep a temp-file name from escaping its directory.
fn sanitize(key: &str) -> String {
    key.replace(['/', '\\'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, body: &str) -> Item {
        Item { key: key.into(), bytes: body.as_bytes().to_vec() }
    }

    #[tokio::test]
    async fn delivery_lands_the_object_at_its_stable_key() {
        let d = tempfile::tempdir().unwrap();
        let dest = LocalDestination { root: d.path().to_path_buf() };

        let it = item("a/b/thing.json", "hello");
        let got = dest.deliver(&it).await.unwrap();
        assert_eq!(got.bytes_written, 5);
        dest.verify(&it, &got).await.unwrap();

        assert_eq!(std::fs::read_to_string(d.path().join("a/b/thing.json")).unwrap(), "hello");
    }

    #[tokio::test]
    async fn redelivery_overwrites_rather_than_duplicating() {
        // This is what makes retry safe. If a redelivery could duplicate, a sink could not retry.
        let d = tempfile::tempdir().unwrap();
        let dest = LocalDestination { root: d.path().to_path_buf() };

        dest.deliver(&item("thing.json", "first")).await.unwrap();
        let second = item("thing.json", "second");
        let got = dest.deliver(&second).await.unwrap();
        dest.verify(&second, &got).await.unwrap();

        assert_eq!(std::fs::read_to_string(d.path().join("thing.json")).unwrap(), "second");
        // One object, not two.
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn no_partial_file_is_left_behind() {
        let d = tempfile::tempdir().unwrap();
        let dest = LocalDestination { root: d.path().to_path_buf() };
        dest.deliver(&item("thing.json", "hello")).await.unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("partial"))
            .collect();
        assert!(leftovers.is_empty(), "the temp file must be renamed, not left: {leftovers:?}");
    }

    #[tokio::test]
    async fn verify_refuses_a_mismatch_so_the_source_is_never_released() {
        let d = tempfile::tempdir().unwrap();
        let dest = LocalDestination { root: d.path().to_path_buf() };
        let it = item("thing.json", "hello");
        dest.deliver(&it).await.unwrap();

        // Claim we wrote more than we did: verify must catch it.
        let lying = Delivered { bytes_written: 999 };
        assert!(dest.verify(&it, &lying).await.is_err());
    }

    #[test]
    fn error_classification_decides_whether_retrying_can_help() {
        assert!(DeliverError::Transient(anyhow::anyhow!("timeout")).is_transient());
        assert!(!DeliverError::Permanent(anyhow::anyhow!("bad credentials")).is_transient());
    }

    #[test]
    fn a_destination_is_built_from_config() {
        let cfg: DestinationConfig =
            serde_json::from_value(serde_json::json!({ "type": "local", "path": "/tmp/out" }))
                .unwrap();
        assert_eq!(build(&cfg).unwrap().kind(), "local");
    }
}
