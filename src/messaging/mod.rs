//! # Messaging
//!
//! **One-liner purpose**: Transport-agnostic messaging — publish/subscribe and
//! request/reply — over a pluggable provider.
//!
//! ## Overview
//! The subsystem has two layers:
//! 1. [`MessagingProvider`] — raw transport primitives (publish / subscribe /
//!    unsubscribe) over [`Destination::Local`] or [`Destination::IotCore`]. The
//!    standalone implementation is [`provider::mqtt`] (dual-broker MQTT via
//!    `rumqttc`); the Greengrass IPC provider lands in Phase 2.
//! 2. [`MessagingService`] — built **once** over any provider; owns [`message::Message`]
//!    (de)serialization and request/reply correlation. Because correlation lives
//!    above the transport, it is identical over MQTT and IPC.
//!
//! ## Semantics & Architecture
//! - Async throughout (`tokio`); traits are object-safe via `async_trait` so they
//!   can be held as `Arc<dyn _>` (the testable seam).
//! - [`Subscription`] cleans up on `Drop` (RAII): dropping it deregisters the
//!   routing entry, so request/reply reply-subscriptions never leak.
//! - Error handling: all fallible operations return [`crate::error::Result`]; the
//!   library never panics on transport errors.
//!
//! ## Usage Example
//! ```no_run
//! use ggcommons::messaging::{Destination, MessagingService};
//! use ggcommons::messaging::message::MessageBuilder;
//! use std::time::Duration;
//!
//! # async fn demo(svc: std::sync::Arc<dyn MessagingService>) -> ggcommons::Result<()> {
//! let req = MessageBuilder::new("Ping", "1.0").thing_name("t").build();
//! let reply = svc.request("svc/ping", req, Destination::Local, Duration::from_secs(5)).await?;
//! println!("got reply: {}", reply.header.name);
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! - Request/reply uses a **dedicated ephemeral reply topic per request** rather
//!   than a shared correlation map: the reply subscription's `Drop` guarantees
//!   cleanup with no bookkeeping, structurally avoiding the leak/key-mismatch
//!   bugs found in the Java implementation.
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
pub use service::MessagingService;

use async_trait::async_trait;

use crate::error::Result;

/// Which broker a message targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A live subscription to raw topic payloads.
///
/// Dropping the `Subscription` deregisters it from its provider (RAII), so a
/// reply subscription created for a single request is cleaned up automatically.
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

/// A live subscription that deserializes payloads into [`Message`]s.
///
/// Payloads that fail to parse are logged at WARN and skipped, so a single
/// malformed message cannot stall the stream.
pub struct MessageStream {
    inner: Subscription,
}

impl MessageStream {
    /// Wrap a raw [`Subscription`] in a message-deserializing stream.
    pub fn new(inner: Subscription) -> Self {
        Self { inner }
    }

    /// Await the next `(topic, Message)`; `None` once the provider goes away.
    pub async fn recv(&mut self) -> Option<(String, Message)> {
        while let Some((topic, bytes)) = self.inner.recv().await {
            match Message::from_slice(&bytes) {
                Ok(msg) => return Some((topic, msg)),
                Err(e) => {
                    tracing::warn!(topic = %topic, error = %e, "dropping unparseable message");
                    continue;
                }
            }
        }
        None
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
