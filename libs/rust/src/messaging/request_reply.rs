//! # Messaging — request/reply helpers
//!
//! **One-liner purpose**: Generate the per-request ephemeral reply topics used by
//! the request/reply pattern, and deliver an inbound reply to its waiting request
//! without ever panicking on a late/duplicate reply.
//!
//! ## Overview
//! Each request subscribes to a unique reply topic, publishes the request with
//! that topic as `reply_to`, and awaits the first message on it. The uniqueness of
//! the topic is what correlates the reply — no shared correlation map is needed,
//! and the reply subscription's `Drop` handles cleanup.
//!
//! Because the reply subscription is torn down as soon as the first reply completes
//! the request (or the request is cancelled / times out), a **late or duplicate
//! reply** can still arrive afterwards, after the delivery channel has been closed
//! and removed. This is the Rust analog of the Java NPE where
//! `responseFutures.get(replyTo)` returned `null` after the future was completed and
//! removed. [`try_deliver_reply`] makes that case a logged no-op: a stray reply for
//! an absent/closed request must never panic, never deref a missing entry, and never
//! take down the subscription or the IPC event loop.
//!
//! ## Semantics & Architecture
//! - Pure functions plus one best-effort, non-blocking delivery helper.
//! - No shared state, no `unwrap`/`expect`, no panics — a stray reply is dropped and
//!   logged at debug.
//! - Error handling: infallible (delivery returns a `bool`, not a `Result`).
//!
//! ## Usage Example
//! ```
//! let topic = ggcommons::messaging::request_reply::new_reply_topic();
//! assert!(topic.starts_with("ggcommons/reply-"));
//! ```
//!
//! ## Related Modules
//! - [`crate::messaging::service`] — consumes these topics.

use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use uuid::Uuid;

/// Prefix for all generated reply topics. Matches the Java/Python
/// `MessageHeader.REPLY_MESSAGE_TOPIC_PREFIX` exactly (note the trailing `-`, not `/`)
/// so request/reply interoperates across the three libraries.
pub const REPLY_TOPIC_PREFIX: &str = "ggcommons/reply-";

/// Generate a globally-unique reply topic for a single request.
///
/// # Post-conditions
/// The returned string begins with [`REPLY_TOPIC_PREFIX`] and is unique per call.
pub fn new_reply_topic() -> String {
    format!("{REPLY_TOPIC_PREFIX}{}", Uuid::new_v4())
}

/// Best-effort, non-blocking delivery of an inbound reply to its waiting request.
///
/// # Purpose
/// Hand a `(topic, payload)` reply to the bounded channel that the matching
/// [`crate::messaging::ReplyFuture`] is awaiting. The waiting request is identified
/// purely by the uniqueness of its ephemeral reply topic (the `out` sender is that
/// request's channel) — there is no shared correlation map to look up, so there is
/// no entry that can be missing and dereferenced.
///
/// This is the Rust analog of the Java null-guard on `responseFutures.get(replyTo)`:
/// a **late or duplicate** reply can arrive after the request completed and its
/// reply subscription was torn down, at which point the channel is full (the single
/// slot already holds the first reply) or closed (the receiver was dropped). In
/// every such case this is a logged no-op — it never panics.
///
/// # Semantics & Syntax
/// - **Signature**: `pub fn try_deliver_reply(out: &Sender<(String, Vec<u8>)>, topic: String, payload: Vec<u8>) -> bool`
/// - Returns `true` if the reply was queued for the waiting request, `false` if it
///   was a stray reply (no live, non-full request channel) and was dropped.
///
/// # Post-conditions
/// - On `true`: the reply is in the request's channel; the awaiting future will see it.
/// - On `false`: nothing is delivered; a debug line is logged. No panic, no blocking.
///
/// # Examples
/// ```
/// use ggcommons::messaging::request_reply::try_deliver_reply;
/// let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, Vec<u8>)>(1);
/// assert!(try_deliver_reply(&tx, "ggcommons/reply-1".into(), b"ok".to_vec()));
/// // The single slot is now full; a duplicate/late reply is a logged no-op.
/// assert!(!try_deliver_reply(&tx, "ggcommons/reply-1".into(), b"dup".to_vec()));
/// assert_eq!(rx.try_recv().unwrap().1, b"ok");
/// ```
pub fn try_deliver_reply(
    out: &Sender<(String, Vec<u8>)>,
    topic: String,
    payload: Vec<u8>,
) -> bool {
    match out.try_send((topic.clone(), payload)) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            // The reply slot already holds this request's reply; this is a late or
            // duplicate reply. Drop it (the Rust equivalent of the Java null-guard).
            tracing::debug!(
                topic = %topic,
                "dropping late/duplicate reply: request already has a reply queued"
            );
            false
        }
        Err(TrySendError::Closed(_)) => {
            // The request completed / was cancelled / timed out and its reply
            // subscription was torn down. A reply for an absent request is a no-op.
            tracing::debug!(
                topic = %topic,
                "dropping stray reply: no waiting request for this reply topic"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_topics_are_prefixed_and_unique() {
        let a = new_reply_topic();
        let b = new_reply_topic();
        assert!(a.starts_with(REPLY_TOPIC_PREFIX));
        assert_ne!(a, b);
    }

    #[test]
    fn delivers_a_reply_to_a_waiting_request() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, Vec<u8>)>(1);
        let topic = new_reply_topic();
        assert!(try_deliver_reply(&tx, topic.clone(), b"hello".to_vec()));
        let (got_topic, got_payload) = rx.try_recv().expect("reply should be queued");
        assert_eq!(got_topic, topic);
        assert_eq!(got_payload, b"hello");
    }

    #[test]
    fn stray_reply_for_closed_request_is_a_noop_not_a_panic() {
        // Receiver dropped => the request completed/cancelled/timed out and its reply
        // subscription is gone. A late reply for this absent request must not panic.
        let (tx, rx) = tokio::sync::mpsc::channel::<(String, Vec<u8>)>(1);
        drop(rx);
        let delivered = try_deliver_reply(&tx, new_reply_topic(), b"late".to_vec());
        assert!(!delivered, "a reply for an absent request must be dropped");
    }

    #[test]
    fn duplicate_reply_after_first_is_a_noop_not_a_panic() {
        // The single reply slot models the at-most-one-reply request/reply contract.
        let (tx, _rx) = tokio::sync::mpsc::channel::<(String, Vec<u8>)>(1);
        let topic = new_reply_topic();
        assert!(try_deliver_reply(&tx, topic.clone(), b"first".to_vec()));
        // A duplicate reply (channel full) is dropped, not a panic.
        assert!(!try_deliver_reply(&tx, topic, b"duplicate".to_vec()));
    }

    #[test]
    fn delivering_to_an_unknown_reply_topic_does_not_panic() {
        // There is no global registry to consult; an unknown correlation id simply
        // has no sender. The closest observable case is a closed channel — exercised
        // above. Here we assert the helper is total over arbitrary topic strings.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, Vec<u8>)>(1);
        assert!(try_deliver_reply(&tx, "ggcommons/reply-unknown-id".into(), vec![]));
        assert!(rx.try_recv().is_ok());
    }
}
