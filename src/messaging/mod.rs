//! # Messaging
//!
//! **One-liner purpose**: Transport-agnostic messaging — callback-based
//! publish/subscribe and request/reply — over a pluggable provider.
//!
//! ## Overview
//! The subsystem has two layers:
//! 1. [`MessagingProvider`] — raw transport primitives (publish / subscribe /
//!    unsubscribe) over [`Destination::Local`] or [`Destination::IotCore`]. The
//!    standalone implementation is [`provider::mqtt`] (dual-broker MQTT via
//!    `rumqttc`); the Greengrass IPC provider lands in Phase 2.
//! 2. [`MessagingService`] — built **once** over any provider; owns [`message::Message`]
//!    (de)serialization, the callback dispatch model, and request/reply correlation.
//!
//! ## Semantics & Architecture
//! - **Callback delivery** (matching the Java/Python contract): a subscription
//!   registers a [`service::MessageHandler`] that is *invoked* on each matching
//!   message — there is no polling.
//! - **Internal queue**: the provider pushes matched messages into an unbounded
//!   per-subscription channel ([`Subscription`]) as they arrive, so messages are
//!   never dropped while a handler is busy.
//! - **Per-subscription concurrency**: each subscription sets a max concurrency for
//!   handler invocation. The default of `1` means strictly serial, ordered
//!   processing; `N` allows up to `N` handlers to run at once (order not
//!   guaranteed across concurrent handlers).
//! - Async throughout (`tokio`); traits are object-safe via `async_trait` so they
//!   can be held as `Arc<dyn _>` (the testable seam).
//! - **Cleanup**: subscriptions are tracked internally; stop one with
//!   [`MessagingService::unsubscribe`], which aborts its dispatcher **and** sends an
//!   UNSUBSCRIBE to the broker (no orphaned broker subscriptions). All
//!   subscriptions are stopped when the service is dropped. (Dropping the raw
//!   internal [`Subscription`] deregisters local routing.)
//! - Error handling: all fallible operations return [`crate::error::Result`]; the
//!   library never panics on transport errors.
//!
//! ## Usage Example
//! ```no_run
//! use ggcommons::messaging::{message_handler, Destination, MessagingService};
//! use std::sync::Arc;
//!
//! # async fn demo(svc: Arc<dyn MessagingService>) -> ggcommons::Result<()> {
//! // Serial processing (max_concurrency = 1): handlers run one at a time, in order.
//! svc.subscribe(
//!     "requests/process",
//!     Destination::Local,
//!     1,
//!     message_handler(|topic, msg| async move {
//!         println!("got {} on {topic}", msg.header.name);
//!     }),
//! )
//! .await?;
//! // ... later, to stop receiving:
//! svc.unsubscribe("requests/process", Destination::Local).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! - Request/reply uses a **dedicated ephemeral reply topic per request** rather
//!   than a shared correlation map: the reply subscription's `Drop` guarantees
//!   cleanup with no bookkeeping, structurally avoiding the leak/key-mismatch
//!   bugs found in the Java implementation.
//! - Concurrency is bounded by a `tokio` semaphore acquired *before* dispatch, so
//!   `max_concurrency = 1` yields ordered serial processing for free.
//!
//! ## Safety & Panics
//! None in normal operation.
//!
//! ## Related Modules
//! - [`message`], [`service`], [`request_reply`], [`config`], [`provider`].

pub mod config;
pub mod message;
pub mod provider;
pub mod request_reply;
pub mod service;

pub use message::{Message, MessageBuilder};
pub use service::{message_handler, DefaultMessagingService, MessageHandler, MessagingService};

use async_trait::async_trait;

use crate::error::Result;

/// Which broker a message targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    /// Local broker (standalone) or local IPC pub/sub (Greengrass).
    Local,
    /// AWS IoT Core.
    IotCore,
}

/// MQTT-style quality of service (the subset the library uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    AtMostOnce,
    AtLeastOnce,
}

/// A live, raw subscription: the internal queue of `(topic, payload)` pairs that
/// the provider pushes into as messages arrive.
///
/// This is the transport-level primitive consumed by the service's dispatcher; it
/// is not used directly by component authors (who register callbacks instead).
/// Dropping it deregisters the subscription from its provider (RAII).
pub struct Subscription {
    rx: tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
    /// Provider-supplied cleanup guard; runs its `Drop` when the subscription ends.
    _guard: Box<dyn std::any::Any + Send>,
}

impl Subscription {
    /// Construct a subscription from a receiver and a provider cleanup guard.
    ///
    /// Intended for provider implementations.
    pub fn new(
        rx: tokio::sync::mpsc::UnboundedReceiver<(String, Vec<u8>)>,
        guard: Box<dyn std::any::Any + Send>,
    ) -> Self {
        Self { rx, _guard: guard }
    }

    /// Await the next `(topic, payload)`; `None` once the provider goes away.
    pub async fn recv(&mut self) -> Option<(String, Vec<u8>)> {
        self.rx.recv().await
    }
}

/// Transport primitives. Implemented by the MQTT (standalone) and IPC (Greengrass)
/// providers.
#[async_trait]
pub trait MessagingProvider: Send + Sync {
    /// Publish raw bytes to `topic` on `dest` at `qos`.
    async fn publish(&self, topic: &str, payload: Vec<u8>, dest: Destination, qos: Qos)
        -> Result<()>;

    /// Subscribe to `filter` on `dest`, returning a [`Subscription`] of raw payloads.
    async fn subscribe(&self, filter: &str, dest: Destination, qos: Qos) -> Result<Subscription>;

    /// Unsubscribe from `filter` on `dest`.
    async fn unsubscribe(&self, filter: &str, dest: Destination) -> Result<()>;
}

/// Test whether an MQTT topic `filter` (with `+` / `#` wildcards) matches a
/// concrete `topic`.
///
/// # Purpose
/// Route incoming publishes to the subscriptions whose filters match, on the
/// client side. Replaces the Java matcher, whose validation was disabled.
///
/// # Semantics & Syntax
/// - **Signature**: `pub fn topic_matches(filter: &str, topic: &str) -> bool`
/// - `+` matches exactly one level; `#` matches the remaining levels (including
///   the parent level) and must be the final segment of a valid filter.
///
/// # Examples
/// ```
/// use ggcommons::messaging::topic_matches;
/// assert!(topic_matches("a/+/c", "a/b/c"));
/// assert!(topic_matches("a/#", "a/b/c"));
/// assert!(topic_matches("a/#", "a"));
/// assert!(!topic_matches("a/+", "a"));
/// assert!(!topic_matches("a/b", "a/b/c"));
/// ```
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();

    for (i, seg) in f.iter().enumerate() {
        match *seg {
            "#" => return true,
            "+" => {
                if i >= t.len() {
                    return false;
                }
            }
            literal => {
                if i >= t.len() || t[i] != literal {
                    return false;
                }
            }
        }
    }
    f.len() == t.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_and_wildcards() {
        assert!(topic_matches("a/b/c", "a/b/c"));
        assert!(topic_matches("a/+/c", "a/b/c"));
        assert!(topic_matches("a/#", "a/b/c"));
        assert!(topic_matches("a/#", "a")); // '#' covers the parent level
        assert!(topic_matches("#", "a/b"));
    }

    #[test]
    fn rejects_non_matches() {
        assert!(!topic_matches("a/b", "a/b/c"));
        assert!(!topic_matches("a/b/c", "a/b"));
        assert!(!topic_matches("a/+", "a")); // '+' requires a level to be present
        assert!(!topic_matches("a/+/c", "a/b/d"));
    }
}
