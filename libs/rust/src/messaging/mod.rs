//! # Messaging
//!
//! **One-liner purpose**: Transport-agnostic messaging — callback-based
//! publish/subscribe and request/reply — mirroring the Java/Python
//! `IMessagingService` contract (explicit local vs northbound method pairs).
//!
//! ## Overview
//! The subsystem has two layers:
//! 1. [`MessagingProvider`] — raw transport primitives (publish / subscribe /
//!    unsubscribe). To keep the transport extensible it takes a [`Destination`]
//!    argument; the standalone implementation is [`provider::mqtt`] (dual-broker
//!    MQTT via `rumqttc`) and the Greengrass implementation is [`provider::ipc`]
//!    (behind the `greengrass` feature).
//! 2. [`MessagingService`] — the user-facing contract, built **once** over any
//!    provider. It exposes **explicit local / northbound method pairs**, owns [`message::Message`]
//!    (de)serialization, the callback dispatch model, and request/reply.
//!
//! ## Semantics & Architecture
//! - **Callback delivery** (Java/Python contract): a subscription registers a
//!   [`service::MessageHandler`] invoked on each matching message — no polling.
//! - **Two per-subscription settings** (mirroring/correcting the Java model, which
//!   conflated them): `max_messages` is the bounded client-side queue capacity
//!   (prevents memory overflow; the oldest-style backpressure is "drop when full"
//!   with a warning), and `max_concurrency` is how many queued messages a
//!   subscription processes at once (`1` = serial, ordered).
//! - **Stopping**: [`MessagingService::unsubscribe`] /
//!   [`MessagingService::unsubscribe_northbound`] abort the dispatcher **and**
//!   UNSUBSCRIBE at the broker; the service stops all dispatchers on drop.
//! - **Request/reply** returns a [`service::ReplyFuture`] handle (the Rust analog of
//!   Java's `CompletableFuture` / Python's `Iou`) carrying a **framework-owned
//!   deadline** (`messaging.requestTimeoutSeconds`, default 30 s,
//!   UNS-CANONICAL-DESIGN §5): await it for the reply or
//!   `Err(EdgeCommonsError::RequestTimeout)`, or cancel via
//!   [`MessagingService::cancel_request`]. Completing, cancelling, timing out, or
//!   dropping the future all clean up the ephemeral reply subscription at the
//!   broker — even when the future is never polled. Per-call override:
//!   [`MessagingService::request_with_timeout`] (`Some(ZERO)` disables).
//! - **Reserved-class publish guard** (§4.1): every client-chosen publish topic
//!   (`publish*`, `request*`, `reply*`) is rejected when it targets a library-owned
//!   UNS class (`state | metric | cfg | log`); `subscribe*` is never guarded. The
//!   library's own publishers use the crate-private reserved seam (§4.2).
//! - Async throughout (`tokio`); traits are object-safe via `async_trait`.
//! - Error handling: [`crate::error::Result`]; never panics on transport errors.
//!
//! ## Usage Example
//! ```no_run
//! use edgecommons::messaging::{message_handler, MessagingService};
//! use std::sync::Arc;
//!
//! # async fn demo(svc: Arc<dyn MessagingService>) -> edgecommons::Result<()> {
//! svc.subscribe(
//!     "requests/process",
//!     message_handler(|topic, msg| async move {
//!         println!("got {} on {topic}", msg.header.name);
//!     }),
//!     32, // max_messages: bounded client-side queue
//!     1,  // max_concurrency: 1 = serial, ordered
//! )
//! .await?;
//! // ... later:
//! svc.unsubscribe("requests/process").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Design Choices
//! - The user-facing service mirrors the Java/Python explicit-pair surface
//!   (`publish`/`publish_northbound`, etc.) intentionally; the lower-level provider
//!   keeps a `Destination` argument as an internal transport detail.
//! - Request/reply uses a dedicated ephemeral reply topic per request; cleanup runs
//!   on completion, cancel, or timeout.
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

pub use message::{Message, MessageBuilder, MessageIdentity};
/// The crate-private reserved-publish seam (UNS-CANONICAL-DESIGN §4.2, D-U4) —
/// re-exported for the library's own publishers (heartbeat/metrics/cfg).
pub(crate) use service::ReservedMessaging;
pub use service::{
    DefaultMessagingService, MessageHandler, MessagingService, ReplyFuture, message_handler,
};

use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;

use crate::error::{EdgeCommonsError, Result};

/// Which broker a message targets. Used by the lower-level [`MessagingProvider`];
/// the user-facing [`MessagingService`] exposes explicit method pairs instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    /// Local broker (standalone) or local IPC pub/sub (Greengrass).
    Local,
    /// The northbound transport.
    Northbound,
}

/// MQTT-style quality of service (the subset the library uses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    AtMostOnce,
    AtLeastOnce,
    /// MQTT QoS 2. Supported only by the standalone local MQTT provider; AWS IoT
    /// Core / Greengrass IoT-Core APIs expose QoS 0/1 only.
    ExactlyOnce,
}

impl Qos {
    /// Convert a numeric MQTT QoS value into the shared enum.
    ///
    /// # Errors
    /// Returns a config error when `value` is not 0, 1, or 2.
    pub fn from_mqtt_value(value: u8, field: &str) -> Result<Self> {
        match value {
            0 => Ok(Self::AtMostOnce),
            1 => Ok(Self::AtLeastOnce),
            2 => Ok(Self::ExactlyOnce),
            other => Err(crate::error::EdgeCommonsError::Config(format!(
                "{field} must be 0..2 (got {other})"
            ))),
        }
    }
}

/// A live, raw subscription: the **bounded** internal queue of `(topic, payload)`
/// pairs that the provider pushes into as messages arrive.
///
/// The capacity is the subscription's `max_messages`. When the queue is full the
/// provider drops the message (with a warning) rather than blocking the shared
/// event loop. Dropping the subscription deregisters it from its provider (RAII).
/// This is the transport-level primitive consumed by the service dispatcher; it is
/// not used directly by component authors.
pub struct Subscription {
    rx: tokio::sync::mpsc::Receiver<(String, Vec<u8>)>,
    /// Provider-supplied cleanup guard; runs its `Drop` when the subscription ends.
    _guard: Box<dyn std::any::Any + Send>,
}

impl Subscription {
    /// Construct a subscription from a bounded receiver and a provider cleanup guard.
    ///
    /// Intended for provider implementations.
    pub fn new(
        rx: tokio::sync::mpsc::Receiver<(String, Vec<u8>)>,
        guard: Box<dyn std::any::Any + Send>,
    ) -> Self {
        Self { rx, _guard: guard }
    }

    /// Await the next `(topic, payload)`; `None` once the provider goes away.
    pub async fn recv(&mut self) -> Option<(String, Vec<u8>)> {
        self.rx.recv().await
    }

    /// Poll for the next `(topic, payload)`. Used by [`crate::messaging::ReplyFuture`].
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<(String, Vec<u8>)>> {
        self.rx.poll_recv(cx)
    }
}

/// Transport primitives. Implemented by the MQTT (standalone) and IPC (Greengrass)
/// providers.
#[async_trait]
pub trait MessagingProvider: Send + Sync {
    /// Publish raw bytes to `topic` on `dest` at `qos`.
    async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
    ) -> Result<()>;

    /// Publish raw bytes and return only after the transport confirms QoS 1 delivery.
    ///
    /// A successful return is strict: MQTT providers must observe the matching broker PUBACK;
    /// Greengrass providers must await successful completion of the IPC publish operation. Queueing
    /// the request locally is not confirmation. Timeout or disconnect is an ambiguous failure.
    ///
    /// Providers which cannot prove this contract deliberately return an error. They must never
    /// delegate to [`Self::publish`] and report its enqueue success as confirmation.
    async fn publish_confirmed(
        &self,
        _topic: &str,
        _payload: Vec<u8>,
        _dest: Destination,
        _qos: Qos,
        _timeout: Duration,
    ) -> Result<()> {
        Err(EdgeCommonsError::Messaging(
            "confirmed publish is not supported by this messaging provider".to_string(),
        ))
    }

    /// Subscribe to `filter` on `dest`, returning a [`Subscription`] whose internal
    /// queue is bounded to `max_messages` entries.
    async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
    ) -> Result<Subscription>;

    /// Subscribe and return only after the transport positively acknowledges activation.
    ///
    /// MQTT implementations must observe a successful SUBACK; Greengrass implementations must
    /// await subscription-operation completion. Unsupported providers fail closed.
    async fn subscribe_acknowledged(
        &self,
        _filter: &str,
        _dest: Destination,
        _qos: Qos,
        _max_messages: usize,
        _timeout: Duration,
    ) -> Result<Subscription> {
        Err(EdgeCommonsError::Messaging(
            "acknowledged subscribe is not supported by this messaging provider".to_string(),
        ))
    }

    /// Unsubscribe from `filter` on `dest`.
    async fn unsubscribe(&self, filter: &str, dest: Destination) -> Result<()>;

    /// Whether the underlying transport currently has a live connection.
    ///
    /// Reads the provider's live connection state (e.g. the MQTT CONNACK watch channel for the
    /// local broker; `true` once the Greengrass IPC client is built). Consumed by the health
    /// readiness endpoint (`/readyz`, [`crate::health`]); it MUST NOT gate liveness (`/livez`),
    /// because a broker outage must never trigger a restart storm.
    fn connected(&self) -> bool;
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
/// use edgecommons::messaging::topic_matches;
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
