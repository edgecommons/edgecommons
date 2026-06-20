//! # Messaging — request/reply helpers
//!
//! **One-liner purpose**: Generate the per-request ephemeral reply topics used by
//! the request/reply pattern.
//!
//! ## Overview
//! Each request subscribes to a unique reply topic, publishes the request with
//! that topic as `reply_to`, and awaits the first message on it. The uniqueness of
//! the topic is what correlates the reply — no shared correlation map is needed,
//! and the reply subscription's `Drop` handles cleanup.
//!
//! ## Semantics & Architecture
//! - Pure functions; no async, no shared state, no panics.
//! - Error handling: not applicable (infallible).
//!
//! ## Usage Example
//! ```
//! let topic = ggcommons::messaging::request_reply::new_reply_topic();
//! assert!(topic.starts_with("ggcommons/reply-"));
//! ```
//!
//! ## Related Modules
//! - [`crate::messaging::service`] — consumes these topics.

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
}
