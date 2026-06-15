//! # Messaging — Message model
//!
//! **One-liner purpose**: The `Message` value type (header + tags + body) and its
//! fluent [`MessageBuilder`], plus JSON (de)serialization for the wire.
//!
//! ## Overview
//! A [`Message`] is the unit exchanged over any transport. Its JSON shape is kept
//! compatible with the Java and Python libraries so the three implementations
//! interoperate on the same topics:
//!
//! ```json
//! { "header": { "name", "version", "timestamp", "correlationId", "uuid", "replyTo" },
//!   "tags":   { "thing": "<thingName>", "...": "..." },
//!   "body":   <any JSON> }
//! ```
//!
//! ## Semantics & Architecture
//! - Messages are plain owned value types: `Clone`, `Send`, `Sync`. There is no
//!   shared mutable state and no interior mutability, so a message handed to
//!   several tasks cannot race (a deliberate contrast with the Java version,
//!   whose `MessageTags.toDict()` mutated shared state).
//! - The correlation id and uuid are assigned **at construction**, never lazily in
//!   a getter.
//! - Error handling: serialization errors surface as [`crate::error::GgError::Json`];
//!   this module performs no I/O.
//!
//! ## Usage Example
//! ```rust
//! use ggcommons::messaging::message::MessageBuilder;
//! use serde_json::json;
//!
//! let msg = MessageBuilder::new("ProcessData", "1.0")
//!     .payload(json!({ "value": 42 }))
//!     .thing_name("my-thing")
//!     .build();
//! assert_eq!(msg.header.name, "ProcessData");
//! let bytes = msg.to_vec().unwrap();
//! let round_tripped = ggcommons::messaging::message::Message::from_slice(&bytes).unwrap();
//! assert_eq!(round_tripped.header.name, "ProcessData");
//! ```
//!
//! ## Design Choices
//! - Wire shape is matched to the existing libraries (cross-language parity rule)
//!   rather than a Rust-optimal shape.
//! - Timestamps are RFC3339 strings (via the `time` crate) to match Java's ISO
//!   instant, not epoch integers.
//!
//! ## Safety & Panics
//! None. All operations are fallible via `Result` rather than panicking.
//!
//! ## Related Modules
//! - [`crate::messaging::service`] — uses messages for publish / request / reply.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::model::Config;
use crate::error::Result;

/// Message metadata. Field names serialize as camelCase for cross-language parity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageHeader {
    /// Logical message name (e.g. `"Heartbeat"`).
    pub name: String,
    /// Message schema version (e.g. `"1.0"`).
    pub version: String,
    /// RFC3339 UTC creation timestamp.
    pub timestamp: String,
    /// Correlation id used to match a reply to its request.
    pub correlation_id: String,
    /// Unique id for this specific message.
    pub uuid: String,
    /// Reply-to topic, present on request messages.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reply_to: Option<String>,
}

/// Message tags: the thing name plus arbitrary string/JSON key-values, serialized
/// flat alongside `"thing"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTags {
    /// IoT Thing name, serialized as `"thing"`.
    #[serde(rename = "thing")]
    pub thing_name: String,
    /// Additional tags, flattened into the same JSON object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A message: header, tags, and an arbitrary JSON body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub header: MessageHeader,
    pub tags: MessageTags,
    pub body: Value,
}

impl Message {
    /// Serialize this message to JSON bytes for the wire.
    ///
    /// # Purpose
    /// Produce the canonical JSON byte representation to publish over a transport.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub fn to_vec(&self) -> Result<Vec<u8>>`
    /// - Borrows `self`; allocates a new `Vec<u8>`.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Json` | The body contains a value serde cannot serialize | Ensure the body is valid JSON |
    pub fn to_vec(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Deserialize a message from JSON bytes received off the wire.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub fn from_slice(bytes: &[u8]) -> Result<Message>`
    /// - Borrows the input slice; returns an owned `Message`.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Json` | Bytes are not valid JSON or do not match the message shape | Validate the producer's format |
    pub fn from_slice(bytes: &[u8]) -> Result<Message> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// The correlation id of this message.
    pub fn correlation_id(&self) -> &str {
        &self.header.correlation_id
    }
}

/// Fluent builder for [`Message`] (the supported construction path).
///
/// `new` stamps a fresh `uuid`, `correlation_id`, and RFC3339 `timestamp`; the
/// remaining fields default to empty until set.
#[derive(Debug, Clone)]
pub struct MessageBuilder {
    header: MessageHeader,
    thing_name: String,
    extra: BTreeMap<String, Value>,
    body: Value,
}

impl MessageBuilder {
    /// Start building a message with the given name and version.
    ///
    /// # Post-conditions
    /// `uuid` and `correlation_id` are populated with fresh v4 UUIDs and
    /// `timestamp` with the current UTC time in RFC3339.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            header: MessageHeader {
                name: name.into(),
                version: version.into(),
                timestamp: now_rfc3339(),
                correlation_id: Uuid::new_v4().to_string(),
                uuid: Uuid::new_v4().to_string(),
                reply_to: None,
            },
            thing_name: String::new(),
            extra: BTreeMap::new(),
            body: Value::Null,
        }
    }

    /// Set the message body.
    pub fn payload(mut self, body: Value) -> Self {
        self.body = body;
        self
    }

    /// Override the correlation id (e.g. to correlate a reply with its request).
    pub fn correlation_id(mut self, id: impl Into<String>) -> Self {
        self.header.correlation_id = id.into();
        self
    }

    /// Set the reply-to topic, marking this as a request.
    pub fn reply_to(mut self, topic: impl Into<String>) -> Self {
        self.header.reply_to = Some(topic.into());
        self
    }

    /// Set the thing name carried in the tags.
    pub fn thing_name(mut self, thing: impl Into<String>) -> Self {
        self.thing_name = thing.into();
        self
    }

    /// Add a single tag.
    pub fn tag(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// Populate the thing name and tags from a configuration snapshot.
    ///
    /// # Semantics & Syntax
    /// - **Signature**: `pub fn from_config(self, config: &Config) -> Self`
    /// - Copies `config.thing_name` and every entry of `config.parsed.tags`.
    pub fn from_config(mut self, config: &Config) -> Self {
        self.thing_name = config.thing_name.clone();
        for (k, v) in &config.parsed.tags {
            self.extra.insert(k.clone(), v.clone());
        }
        self
    }

    /// Finalize the message.
    pub fn build(self) -> Message {
        Message {
            header: self.header,
            tags: MessageTags {
                thing_name: self.thing_name,
                extra: self.extra,
            },
            body: self.body,
        }
    }
}

/// Current UTC time formatted as RFC3339, or a fixed epoch string on the
/// (practically impossible) formatting failure.
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builder_stamps_identity_fields() {
        let m = MessageBuilder::new("N", "1.0").build();
        assert!(!m.header.uuid.is_empty());
        assert!(!m.header.correlation_id.is_empty());
        assert!(m.header.timestamp.contains('T'));
        assert!(m.header.reply_to.is_none());
    }

    #[test]
    fn round_trips_through_json_with_expected_shape() {
        let m = MessageBuilder::new("ProcessData", "1.0")
            .payload(json!({ "v": 42 }))
            .thing_name("thing-1")
            .tag("site", json!("factory-1"))
            .correlation_id("corr-123")
            .reply_to("reply/here")
            .build();

        let bytes = m.to_vec().unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["header"]["name"], "ProcessData");
        assert_eq!(value["header"]["correlationId"], "corr-123");
        assert_eq!(value["header"]["replyTo"], "reply/here");
        assert_eq!(value["tags"]["thing"], "thing-1");
        assert_eq!(value["tags"]["site"], "factory-1");
        assert_eq!(value["body"]["v"], 42);

        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn reply_to_is_omitted_when_absent() {
        let m = MessageBuilder::new("N", "1.0").build();
        let value: Value = serde_json::from_slice(&m.to_vec().unwrap()).unwrap();
        assert!(value["header"].get("replyTo").is_none());
    }

    #[test]
    fn from_config_copies_thing_and_tags() {
        let cfg = Config::from_value("c", "thing-9", json!({ "tags": { "site": "f1" } })).unwrap();
        let m = MessageBuilder::new("N", "1.0").from_config(&cfg).build();
        assert_eq!(m.tags.thing_name, "thing-9");
        assert_eq!(m.tags.extra.get("site"), Some(&json!("f1")));
    }
}
