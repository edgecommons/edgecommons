//! # Messaging — Message model
//!
//! **One-liner purpose**: The `Message` value type (header + identity + tags + body)
//! and its fluent [`MessageBuilder`], plus JSON (de)serialization for the wire.
//!
//! ## Overview
//! A [`Message`] is the unit exchanged over any transport. Its JSON shape is kept
//! compatible with the Java (canonical), Python, and TypeScript libraries so the four
//! implementations interoperate on the same topics:
//!
//! ```json
//! { "header":   { "name", "version", "timestamp", "correlation_id", "uuid", "reply_to" },
//!   "identity": { "hier": [ { "level", "value" } ], "path", "component", "instance" },
//!   "tags":     { "...": "..." },
//!   "body":     <any JSON> }
//! ```
//!
//! Header keys are **snake_case** (`correlation_id`, `reply_to`) to match the
//! Java/Python `MessageHeader.toDict()` wire format exactly. The canonical envelope
//! member order is `header`, `identity`, `tags`, `body` (member order is NOT
//! normative — envelopes compare structurally, D-U22).
//!
//! The top-level [`MessageIdentity`] element (UNS-CANONICAL-DESIGN §1) carries the
//! publisher's enterprise-hierarchy identity: the ordered `hier` levels (last entry =
//! the device), the precomputed `path`, the `component` UNS token, and the
//! per-message `instance` (default `"main"`). It is **optional on the wire**: a
//! message built without a config-bound builder (the CONFIG_COMPONENT bootstrap
//! request, raw bridging of external systems) legally omits it.
//!
//! **UNS hard cut:** the synthesized `tags.thing` member is GONE — the device now
//! travels in `identity` (its last `hier` entry). A stray inbound `thing` key lands
//! in the generic tag map like any other tag (no legacy shim).
//!
//! A message can also be **raw** (a non-envelope payload). When a received payload
//! is not an envelope (it has none of `header`/`identity`/`tags`/`body`, or is not
//! even a JSON object), it is delivered as a raw message carrying the original
//! value, mirroring Java's `Message.getRaw()` / Python's `Message.raw`. A raw
//! message serializes as `{ "raw": <value> }` and never carries an identity.
//!
//! ## Semantics & Architecture
//! - Messages are plain owned value types: `Clone`, `Send`, `Sync`.
//! - The correlation id and uuid are assigned **at construction** (pin them with
//!   [`MessageBuilder::uuid`] / [`MessageBuilder::timestamp`] for deterministic
//!   envelopes — tests and the cross-language `uns-test-vectors` goldens, D-U13).
//! - Deserialization of `identity` is deliberately **lenient**: a malformed identity
//!   yields `None` plus a WARN log and the message still delivers (mirroring the
//!   lenient envelope handling across all four libraries).
//!
//! ## Usage Example
//! ```rust
//! use ggcommons::messaging::message::MessageBuilder;
//! use serde_json::json;
//!
//! let msg = MessageBuilder::new("ProcessData", "1.0")
//!     .payload(json!({ "value": 42 }))
//!     .build();
//! assert_eq!(msg.header.name, "ProcessData");
//! let bytes = msg.to_vec().unwrap();
//! let round_tripped = ggcommons::messaging::message::Message::from_slice(&bytes).unwrap();
//! assert_eq!(round_tripped.header.name, "ProcessData");
//! ```
//!
//! ## Related Modules
//! - [`crate::messaging::service`] — uses messages for publish / request / reply.
//! - [`crate::uns`] — builds the topics these envelopes are published on.

use std::collections::BTreeMap;

use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::model::Config;
use crate::error::{GgError, Result};

/// Message metadata. Field names serialize as snake_case (`correlation_id`,
/// `reply_to`) to match the Java/Python `MessageHeader` wire format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageHeader {
    /// Logical message name (e.g. `"state"`).
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

/// One level of the enterprise hierarchy inside a [`MessageIdentity`]: the level's
/// configured `level` name and this deployment's `value` for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HierEntry {
    /// The hierarchy level name (e.g. `"site"`, `"device"`).
    pub level: String,
    /// This deployment's value for the level (e.g. `"dallas"`, `"gw-01"`).
    pub value: String,
}

/// The top-level `identity` envelope element of the unified namespace
/// (UNS-CANONICAL-DESIGN §1).
///
/// One immutable type serves as both the wire object and the component's resolved
/// identity (see [`Config::identity`](crate::config::model::Config::identity)):
/// - `hier` — the ordered enterprise hierarchy (len ≥ 1); its **last entry is always
///   the physical device**. There is no standalone `device` wire field —
///   [`Self::device`] is a computed accessor over the last entry.
/// - `path` — the precomputed `'/'`-join of the `hier` values. The publisher is
///   authoritative: on deserialize a present `path` is taken as-is, a missing one is
///   recomputed.
/// - `component` — the publishing component's UNS token (the sanitized short name,
///   the existing `{ComponentName}` semantics — D-U18).
/// - `instance` — the per-message instance token, never empty (default
///   [`Self::DEFAULT_INSTANCE`]).
///
/// Serialization emits the canonical member order `hier, path, component, instance`
/// (field order = emit order). Deserialization ([`Self::from_wire`]) is deliberately
/// lenient: a malformed identity yields `None` plus a WARN log and the enclosing
/// message still delivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageIdentity {
    hier: Vec<HierEntry>,
    path: String,
    component: String,
    instance: String,
}

impl MessageIdentity {
    /// The default per-message instance token, used when no instance is specified.
    pub const DEFAULT_INSTANCE: &'static str = "main";

    /// Creates a validated identity, precomputing `path` as the `'/'`-join of the
    /// `hier` values. An absent/empty `instance` defaults to
    /// [`Self::DEFAULT_INSTANCE`].
    ///
    /// # Errors
    /// [`GgError::Messaging`] when `hier` is empty, an entry's level/value is empty,
    /// or `component` is empty.
    pub fn new(
        hier: Vec<HierEntry>,
        component: impl Into<String>,
        instance: Option<String>,
    ) -> Result<MessageIdentity> {
        let component = component.into();
        if hier.is_empty() {
            return Err(GgError::Messaging(
                "MessageIdentity hier must contain at least one entry".to_string(),
            ));
        }
        for entry in &hier {
            if entry.level.is_empty() {
                return Err(GgError::Messaging(
                    "MessageIdentity hier entry level must be non-empty".to_string(),
                ));
            }
            if entry.value.is_empty() {
                return Err(GgError::Messaging(format!(
                    "MessageIdentity hier entry value for level '{}' must be non-empty",
                    entry.level
                )));
            }
        }
        if component.is_empty() {
            return Err(GgError::Messaging(
                "MessageIdentity component must be non-empty".to_string(),
            ));
        }
        let path = hier.iter().map(|e| e.value.as_str()).collect::<Vec<_>>().join("/");
        let instance = match instance {
            Some(i) if !i.is_empty() => i,
            _ => Self::DEFAULT_INSTANCE.to_string(),
        };
        Ok(MessageIdentity { hier, path, component, instance })
    }

    /// Returns the ordered hierarchy entries (the last entry is the device).
    pub fn hier(&self) -> &[HierEntry] {
        &self.hier
    }

    /// Returns the precomputed `'/'`-join of the hierarchy values.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the component UNS token (the sanitized short name).
    pub fn component(&self) -> &str {
        &self.component
    }

    /// Returns the per-message instance token (never empty).
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// Computed accessor — the last `hier` entry's value. NOT a wire field: the
    /// device is inherent to the hierarchy (its deepest level), so it is never
    /// serialized separately.
    pub fn device(&self) -> &str {
        &self.hier[self.hier.len() - 1].value
    }

    /// Returns a copy of this identity with a different per-message instance token.
    ///
    /// # Errors
    /// [`GgError::Messaging`] when `instance` is empty.
    pub fn with_instance(&self, instance: impl Into<String>) -> Result<MessageIdentity> {
        let instance = instance.into();
        if instance.is_empty() {
            return Err(GgError::Messaging(
                "MessageIdentity instance must be non-empty".to_string(),
            ));
        }
        Ok(MessageIdentity {
            hier: self.hier.clone(),
            path: self.path.clone(),
            component: self.component.clone(),
            instance,
        })
    }

    /// Infallible internal variant of [`Self::with_instance`] for the builder's
    /// stamping site (empty falls back to [`Self::DEFAULT_INSTANCE`]).
    pub(crate) fn with_instance_or_default(&self, instance: &str) -> MessageIdentity {
        let instance =
            if instance.is_empty() { Self::DEFAULT_INSTANCE } else { instance }.to_string();
        MessageIdentity {
            hier: self.hier.clone(),
            path: self.path.clone(),
            component: self.component.clone(),
            instance,
        }
    }

    /// Lenient wire-form parser (mirrors Java `MessageIdentity.fromDict`): a missing
    /// `instance` defaults to [`Self::DEFAULT_INSTANCE`]; a missing `path` is
    /// recomputed from the hier values (a present one is taken as-is — the publisher
    /// is authoritative); a malformed identity (non-object element,
    /// missing/empty/non-array `hier`, malformed hier entries, or a missing
    /// `component`) yields `None` plus a WARN log so the enclosing message still
    /// delivers.
    pub fn from_wire(src: &Value) -> Option<MessageIdentity> {
        let Some(obj) = src.as_object() else {
            tracing::warn!("Malformed message identity: 'identity' is not an object; dropping identity");
            return None;
        };
        let Some(hier_arr) = obj.get("hier").and_then(Value::as_array).filter(|a| !a.is_empty())
        else {
            tracing::warn!(
                "Malformed message identity: 'hier' missing, not an array, or empty; dropping identity"
            );
            return None;
        };
        let mut hier = Vec::with_capacity(hier_arr.len());
        for entry in hier_arr {
            let Some(entry_obj) = entry.as_object() else {
                tracing::warn!("Malformed message identity: hier entry is not an object; dropping identity");
                return None;
            };
            let level = non_empty_str(entry_obj.get("level"));
            let value = non_empty_str(entry_obj.get("value"));
            let (Some(level), Some(value)) = (level, value) else {
                tracing::warn!("Malformed message identity: hier entry missing level/value; dropping identity");
                return None;
            };
            hier.push(HierEntry { level: level.to_string(), value: value.to_string() });
        }
        let Some(component) = non_empty_str(obj.get("component")) else {
            tracing::warn!("Malformed message identity: 'component' missing or empty; dropping identity");
            return None;
        };
        let path = match non_empty_str(obj.get("path")) {
            Some(p) => p.to_string(), // present => taken as-is (publisher is authoritative)
            None => hier.iter().map(|e| e.value.as_str()).collect::<Vec<_>>().join("/"),
        };
        let instance = non_empty_str(obj.get("instance"))
            .unwrap_or(Self::DEFAULT_INSTANCE)
            .to_string();
        Some(MessageIdentity { hier, path, component: component.to_string(), instance })
    }
}

/// The element as a non-empty string, or `None` if absent/non-string/empty.
fn non_empty_str(element: Option<&Value>) -> Option<&str> {
    element.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Message tags: arbitrary business-context key-values serialized flat.
///
/// UNS hard cut: the synthesized `thing` member is gone — the device travels in the
/// top-level [`MessageIdentity`]. A stray inbound `thing` key is an ordinary tag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTags {
    /// The tag map, flattened into the JSON object.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A message: either an **envelope** (header + optional identity + optional tags +
/// body) or a **raw** (non-envelope) payload.
///
/// For an envelope, `header`/`identity`/`tags`/`body` are meaningful and `raw` is
/// `None`. For a raw message, `raw` is `Some(value)` and the other fields hold
/// defaults that should be ignored (check [`Message::is_raw`] / read
/// [`Message::get_raw`]). This mirrors Java's `Message` (`getRaw()`) and Python's
/// `Message.raw`.
///
/// Serialization (matching Java/Python `toDict`): a raw message serializes as
/// `{ "raw": <value> }`; an envelope as `{ "header", "identity"?, "tags"?, "body" }`
/// in the canonical member order (`identity` omitted when `None`, `tags` omitted
/// when `None` — a message built without a config-bound builder carries neither).
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub header: MessageHeader,
    /// The UNS identity element (`hier`/`path`/`component`/`instance`), or `None`
    /// when the message carries none (raw messages, messages built without a
    /// config-bound builder, or a malformed inbound identity).
    pub identity: Option<MessageIdentity>,
    /// The business-context tags, or `None` when the envelope carries no `tags`
    /// member (a message built without a config-bound builder and no explicit tags).
    pub tags: Option<MessageTags>,
    pub body: Value,
    /// When `Some`, this is a raw (non-envelope) message; the other fields are
    /// defaults and should be ignored.
    pub raw: Option<Value>,
}

impl Serialize for Message {
    /// Serialize as `{ "raw": .. }` for a raw message, else as the canonical
    /// `{ "header", "identity"?, "tags"?, "body" }` envelope (matching Java `toDict`).
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        if let Some(raw) = &self.raw {
            let mut map = serializer.serialize_map(Some(1))?;
            map.serialize_entry("raw", raw)?;
            map.end()
        } else {
            let entries =
                2 + usize::from(self.identity.is_some()) + usize::from(self.tags.is_some());
            let mut map = serializer.serialize_map(Some(entries))?;
            map.serialize_entry("header", &self.header)?;
            // Canonical envelope member order: header, identity, tags, body.
            if let Some(identity) = &self.identity {
                map.serialize_entry("identity", identity)?;
            }
            if let Some(tags) = &self.tags {
                map.serialize_entry("tags", tags)?;
            }
            map.serialize_entry("body", &self.body)?;
            map.end()
        }
    }
}

impl<'de> Deserialize<'de> for Message {
    /// Classify the incoming JSON: an object with any of
    /// `header`/`identity`/`tags`/`body` is an envelope (missing parts default);
    /// anything else becomes a raw message carrying the original value (mirroring
    /// Java's `MessageBuilder.fromObject` / Python's `from_object`).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Message::from_json_value(value).map_err(de::Error::custom)
    }
}

impl Message {
    /// Construct a raw (non-envelope) message carrying `value`.
    pub fn raw(value: Value) -> Message {
        Message {
            header: MessageHeader::default(),
            identity: None,
            tags: None,
            body: Value::Null,
            raw: Some(value),
        }
    }

    /// Whether this is a raw (non-envelope) message.
    pub fn is_raw(&self) -> bool {
        self.raw.is_some()
    }

    /// The raw payload, if this is a raw message (`None` for an envelope).
    pub fn get_raw(&self) -> Option<&Value> {
        self.raw.as_ref()
    }

    /// Classify a parsed JSON value into an envelope or a raw message.
    ///
    /// An object carrying any of `header`/`identity`/`tags`/`body` is treated as an
    /// envelope (missing parts default; a malformed `header`/`tags` is an error; a
    /// malformed `identity` is leniently dropped with a WARN). Any other value
    /// (including an object with none of those keys) becomes a raw message.
    fn from_json_value(value: Value) -> Result<Message> {
        if let Value::Object(map) = &value {
            let is_envelope = map.contains_key("header")
                || map.contains_key("identity")
                || map.contains_key("tags")
                || map.contains_key("body");
            if is_envelope {
                let header = match map.get("header") {
                    Some(h) => serde_json::from_value(h.clone())?,
                    None => MessageHeader::default(),
                };
                // Lenient: a malformed identity is dropped (WARN inside from_wire).
                let identity = map.get("identity").and_then(MessageIdentity::from_wire);
                let tags = match map.get("tags") {
                    Some(t) => Some(serde_json::from_value(t.clone())?),
                    None => None,
                };
                let body = map.get("body").cloned().unwrap_or(Value::Null);
                return Ok(Message { header, identity, tags, body, raw: None });
            }
        }
        // Non-envelope (or non-object): deliver as raw, matching Java/Python.
        Ok(Message::raw(value))
    }

    /// Serialize this message to JSON bytes for the wire.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Json` | The body contains a value serde cannot serialize | Ensure the body is valid JSON |
    pub fn to_vec(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Deserialize a message from bytes received off the wire.
    ///
    /// Valid JSON is classified into an envelope or a raw message (see
    /// [`Message::from_json_value`]). Bytes that are **not valid JSON** are delivered
    /// as a raw message carrying the payload as a UTF-8 (lossy) string, so a message
    /// is never silently dropped — matching the Java/Python behavior.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `GgError::Json` | Bytes are valid JSON but a present `header`/`tags` is malformed | Validate the producer's envelope shape |
    pub fn from_slice(bytes: &[u8]) -> Result<Message> {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => Message::from_json_value(value),
            // Not valid JSON: deliver as a raw string rather than dropping it.
            Err(_) => Ok(Message::raw(Value::String(
                String::from_utf8_lossy(bytes).into_owned(),
            ))),
        }
    }

    /// The correlation id of this message.
    pub fn correlation_id(&self) -> &str {
        &self.header.correlation_id
    }
}

/// Fluent builder for [`Message`] (the supported construction path).
///
/// `new` stamps a fresh `uuid`, `correlation_id`, and RFC3339 `timestamp` (pin them
/// with [`Self::uuid`] / [`Self::timestamp`] for deterministic envelopes); the
/// remaining fields default to empty until set.
///
/// **`build()` is the single UNS identity stamping site** (UNS-CANONICAL-DESIGN
/// §1.4): an explicit [`Self::identity`] override wins and is stamped verbatim;
/// otherwise, when the builder is config-bound ([`Self::from_config`]), the
/// component's resolved identity is stamped with the per-message instance token
/// ([`Self::instance`], default [`MessageIdentity::DEFAULT_INSTANCE`]); with
/// neither, `identity` stays `None` (bootstrap/raw messages legally omit it).
#[derive(Debug, Clone)]
pub struct MessageBuilder {
    header: MessageHeader,
    tags: Option<BTreeMap<String, Value>>,
    body: Value,
    config_identity: Option<MessageIdentity>,
    identity_override: Option<MessageIdentity>,
    instance: Option<String>,
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
            tags: None,
            body: Value::Null,
            config_identity: None,
            identity_override: None,
            instance: None,
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

    /// Pin the header `uuid` instead of the generated random one — deterministic
    /// envelopes for tests and the cross-language `uns-test-vectors` golden
    /// envelopes (D-U13).
    pub fn uuid(mut self, uuid: impl Into<String>) -> Self {
        self.header.uuid = uuid.into();
        self
    }

    /// Pin the header `timestamp` instead of the generated "now" — deterministic
    /// envelopes for tests and the cross-language `uns-test-vectors` golden
    /// envelopes (D-U13). RFC3339/ISO-8601 instant by convention.
    pub fn timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.header.timestamp = timestamp.into();
        self
    }

    /// Set the reply-to topic, marking this as a request.
    pub fn reply_to(mut self, topic: impl Into<String>) -> Self {
        self.header.reply_to = Some(topic.into());
        self
    }

    /// Add a single tag (creates the envelope `tags` member if absent).
    pub fn tag(mut self, key: impl Into<String>, value: Value) -> Self {
        self.tags.get_or_insert_with(BTreeMap::new).insert(key.into(), value);
        self
    }

    /// Set the per-message instance token stamped into the identity element
    /// (default [`MessageIdentity::DEFAULT_INSTANCE`]). Only takes effect when a
    /// config-resolved identity is stamped (an explicit [`Self::identity`] override
    /// is stamped verbatim). An empty token falls back to the default.
    pub fn instance(mut self, instance: impl Into<String>) -> Self {
        self.instance = Some(instance.into());
        self
    }

    /// Set an explicit identity override (tests, conformance vectors, relays). Wins
    /// over the config-resolved identity and is stamped verbatim (the
    /// [`Self::instance`] token is not applied to an override).
    pub fn identity(mut self, identity: MessageIdentity) -> Self {
        self.identity_override = Some(identity);
        self
    }

    /// Bind the builder to a configuration snapshot: captures the component's
    /// resolved UNS identity (stamped by `build()` with the per-message instance
    /// token) and populates the envelope tags from the config `tags` section
    /// (explicit [`Self::tag`] entries win on key conflict).
    pub fn from_config(mut self, config: &Config) -> Self {
        self.config_identity = Some(config.identity().clone());
        let merged = self.tags.take();
        let mut tags: BTreeMap<String, Value> = config.parsed.tags.clone();
        if let Some(explicit) = merged {
            tags.extend(explicit);
        }
        self.tags = Some(tags);
        self
    }

    /// Finalize the message. The single identity stamping site: explicit override >
    /// config-resolved component identity (+ per-message instance token) > none
    /// (bootstrap/raw cases stay valid).
    pub fn build(self) -> Message {
        let identity = if let Some(identity_override) = self.identity_override {
            Some(identity_override)
        } else {
            self.config_identity.map(|component_identity| {
                component_identity.with_instance_or_default(
                    self.instance.as_deref().unwrap_or(MessageIdentity::DEFAULT_INSTANCE),
                )
            })
        };
        Message {
            header: self.header,
            identity,
            tags: self.tags.map(|extra| MessageTags { extra }),
            body: self.body,
            raw: None,
        }
    }
}

/// Current UTC time formatted as RFC3339, or a fixed epoch string on the
/// (practically impossible) formatting failure.
///
/// `pub(crate)`: also reused as the production [`crate::facades::Clock`] seam (the
/// `data()`/`events()` facades' `serverTs`/`timestamp` "now" default) so there is exactly one
/// "current time as our wire timestamp format" function in the crate.
pub(crate) fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_identity() -> MessageIdentity {
        MessageIdentity::new(
            vec![
                HierEntry { level: "site".into(), value: "dallas".into() },
                HierEntry { level: "device".into(), value: "gw-01".into() },
            ],
            "opcua-adapter",
            None,
        )
        .unwrap()
    }

    #[test]
    fn builder_stamps_identity_fields() {
        let m = MessageBuilder::new("N", "1.0").build();
        assert!(!m.header.uuid.is_empty());
        assert!(!m.header.correlation_id.is_empty());
        assert!(m.header.timestamp.contains('T'));
        assert!(m.header.reply_to.is_none());
        assert!(m.identity.is_none(), "no config, no override => no identity");
        assert!(m.tags.is_none(), "no config, no tags => no tags member");
    }

    #[test]
    fn builder_pins_uuid_and_timestamp() {
        let m = MessageBuilder::new("N", "1.0")
            .uuid("00000000-0000-4000-8000-000000000001")
            .timestamp("2026-07-01T12:00:00Z")
            .correlation_id("00000000-0000-4000-8000-000000000002")
            .build();
        assert_eq!(m.header.uuid, "00000000-0000-4000-8000-000000000001");
        assert_eq!(m.header.timestamp, "2026-07-01T12:00:00Z");
        assert_eq!(m.header.correlation_id, "00000000-0000-4000-8000-000000000002");
    }

    #[test]
    fn round_trips_through_json_with_expected_shape() {
        let m = MessageBuilder::new("ProcessData", "1.0")
            .payload(json!({ "v": 42 }))
            .tag("site", json!("factory-1"))
            .correlation_id("corr-123")
            .reply_to("reply/here")
            .build();

        let bytes = m.to_vec().unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["header"]["name"], "ProcessData");
        // Wire keys are snake_case, matching Java/Python.
        assert_eq!(value["header"]["correlation_id"], "corr-123");
        assert_eq!(value["header"]["reply_to"], "reply/here");
        assert_eq!(value["tags"]["site"], "factory-1");
        assert!(value.get("identity").is_none(), "identity omitted when absent");
        assert_eq!(value["body"]["v"], 42);

        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn identity_serializes_between_header_and_tags_and_round_trips() {
        let m = MessageBuilder::new("state", "1.0")
            .identity(test_identity())
            .payload(json!({ "status": "RUNNING" }))
            .build();
        let bytes = m.to_vec().unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        // Canonical member order: header, identity, (tags,) body.
        let header_pos = text.find("\"header\"").unwrap();
        let identity_pos = text.find("\"identity\"").unwrap();
        let body_pos = text.find("\"body\"").unwrap();
        assert!(header_pos < identity_pos && identity_pos < body_pos);

        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["identity"]["path"], "dallas/gw-01");
        assert_eq!(value["identity"]["component"], "opcua-adapter");
        assert_eq!(value["identity"]["instance"], "main");
        assert_eq!(value["identity"]["hier"][1]["value"], "gw-01");

        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back.identity, m.identity);
    }

    #[test]
    fn identity_accessors_and_with_instance() {
        let id = test_identity();
        assert_eq!(id.device(), "gw-01");
        assert_eq!(id.path(), "dallas/gw-01");
        assert_eq!(id.component(), "opcua-adapter");
        assert_eq!(id.instance(), "main");
        let kep = id.with_instance("kep1").unwrap();
        assert_eq!(kep.instance(), "kep1");
        assert_eq!(kep.device(), "gw-01");
        assert!(id.with_instance("").is_err());
    }

    #[test]
    fn identity_constructor_validates() {
        assert!(MessageIdentity::new(vec![], "c", None).is_err(), "empty hier");
        assert!(
            MessageIdentity::new(
                vec![HierEntry { level: "".into(), value: "v".into() }],
                "c",
                None
            )
            .is_err(),
            "empty level"
        );
        assert!(
            MessageIdentity::new(
                vec![HierEntry { level: "device".into(), value: "".into() }],
                "c",
                None
            )
            .is_err(),
            "empty value"
        );
        assert!(
            MessageIdentity::new(
                vec![HierEntry { level: "device".into(), value: "d".into() }],
                "",
                None
            )
            .is_err(),
            "empty component"
        );
    }

    #[test]
    fn lenient_identity_parse_drops_malformed_and_defaults_missing() {
        // Missing instance -> "main"; missing path -> recomputed.
        let parsed = MessageIdentity::from_wire(&json!({
            "hier": [ { "level": "device", "value": "gw-01" } ],
            "component": "c"
        }))
        .unwrap();
        assert_eq!(parsed.instance(), "main");
        assert_eq!(parsed.path(), "gw-01");

        // A present path is authoritative (taken as-is).
        let parsed = MessageIdentity::from_wire(&json!({
            "hier": [ { "level": "device", "value": "gw-01" } ],
            "path": "publisher/said/so",
            "component": "c"
        }))
        .unwrap();
        assert_eq!(parsed.path(), "publisher/said/so");

        // Malformed shapes -> None (message still delivers; see envelope test below).
        assert!(MessageIdentity::from_wire(&json!("not an object")).is_none());
        assert!(MessageIdentity::from_wire(&json!({ "component": "c" })).is_none());
        assert!(MessageIdentity::from_wire(&json!({ "hier": [], "component": "c" })).is_none());
        assert!(MessageIdentity::from_wire(&json!({ "hier": ["bad"], "component": "c" })).is_none());
        assert!(
            MessageIdentity::from_wire(&json!({
                "hier": [ { "level": "device" } ], "component": "c"
            }))
            .is_none()
        );
        assert!(
            MessageIdentity::from_wire(&json!({
                "hier": [ { "level": "device", "value": "gw-01" } ]
            }))
            .is_none()
        );
    }

    #[test]
    fn malformed_inbound_identity_still_delivers_the_message() {
        let bytes = serde_json::to_vec(&json!({
            "header": { "name": "N", "version": "1.0", "timestamp": "t",
                        "correlation_id": "c", "uuid": "u" },
            "identity": { "hier": [], "component": "c" },
            "body": { "v": 1 }
        }))
        .unwrap();
        let m = Message::from_slice(&bytes).unwrap();
        assert!(!m.is_raw());
        assert!(m.identity.is_none(), "malformed identity dropped leniently");
        assert_eq!(m.body["v"], 1);
    }

    #[test]
    fn identity_alone_marks_an_envelope() {
        // The envelope-detection predicate includes `identity` (D-U1 §1.3).
        let bytes = serde_json::to_vec(&json!({
            "identity": {
                "hier": [ { "level": "device", "value": "gw-01" } ],
                "component": "c"
            }
        }))
        .unwrap();
        let m = Message::from_slice(&bytes).unwrap();
        assert!(!m.is_raw(), "an object with an identity member is an envelope");
        assert_eq!(m.identity.unwrap().device(), "gw-01");
    }

    #[test]
    fn stray_thing_tag_is_an_ordinary_tag() {
        // UNS hard cut: no `thing` special-casing — it lands in the generic tag map.
        let bytes = serde_json::to_vec(&json!({
            "header": { "name": "N", "version": "1.0", "timestamp": "t",
                        "correlation_id": "c", "uuid": "u" },
            "tags": { "thing": "legacy-thing", "site": "f1" },
            "body": null
        }))
        .unwrap();
        let m = Message::from_slice(&bytes).unwrap();
        let tags = m.tags.unwrap();
        assert_eq!(tags.extra.get("thing"), Some(&json!("legacy-thing")));
        assert_eq!(tags.extra.get("site"), Some(&json!("f1")));
    }

    #[test]
    fn reply_to_is_omitted_when_absent() {
        let m = MessageBuilder::new("N", "1.0").build();
        let value: Value = serde_json::from_slice(&m.to_vec().unwrap()).unwrap();
        assert!(value["header"].get("reply_to").is_none());
    }

    #[test]
    fn non_envelope_object_is_received_as_raw() {
        // A payload with none of header/identity/tags/body is delivered raw.
        let bytes = serde_json::to_vec(&json!({ "temperature": 21.5, "ok": true })).unwrap();
        let m = Message::from_slice(&bytes).unwrap();
        assert!(m.is_raw());
        assert_eq!(m.get_raw().unwrap()["temperature"], 21.5);
    }

    #[test]
    fn non_json_payload_is_received_as_raw_string() {
        let m = Message::from_slice(b"not json at all").unwrap();
        assert!(m.is_raw());
        assert_eq!(m.get_raw().unwrap(), &json!("not json at all"));
    }

    #[test]
    fn raw_message_serializes_under_raw_key() {
        let m = Message::raw(json!({ "x": 1 }));
        let value: Value = serde_json::from_slice(&m.to_vec().unwrap()).unwrap();
        assert_eq!(value, json!({ "raw": { "x": 1 } }));
    }

    #[test]
    fn envelope_with_missing_parts_defaults_them() {
        // Body-only object is still an envelope (it has the `body` key).
        let bytes = serde_json::to_vec(&json!({ "body": { "v": 1 } })).unwrap();
        let m = Message::from_slice(&bytes).unwrap();
        assert!(!m.is_raw());
        assert_eq!(m.body, json!({ "v": 1 }));
        assert_eq!(m.header, MessageHeader::default());
        assert!(m.identity.is_none());
        assert!(m.tags.is_none());
    }

    #[test]
    fn from_config_stamps_identity_and_tags() {
        let cfg = Config::from_value(
            "com.example.MyComp",
            "thing-9",
            json!({ "tags": { "site": "f1" } }),
        )
        .unwrap();
        let m = MessageBuilder::new("N", "1.0").from_config(&cfg).build();
        let identity = m.identity.expect("config-bound builder stamps identity");
        assert_eq!(identity.device(), "thing-9");
        assert_eq!(identity.component(), "MyComp");
        assert_eq!(identity.instance(), "main");
        let tags = m.tags.expect("config-bound builder stamps tags");
        assert_eq!(tags.extra.get("site"), Some(&json!("f1")));
        assert!(!tags.extra.contains_key("thing"), "no synthesized thing tag (hard cut)");
    }

    #[test]
    fn instance_token_applies_to_config_identity_only() {
        let cfg = Config::from_value("c", "thing-9", json!({})).unwrap();
        let m = MessageBuilder::new("N", "1.0").from_config(&cfg).instance("kep1").build();
        assert_eq!(m.identity.unwrap().instance(), "kep1");

        // An explicit override is stamped verbatim; the instance token is ignored.
        let m = MessageBuilder::new("N", "1.0")
            .from_config(&cfg)
            .identity(test_identity())
            .instance("kep1")
            .build();
        assert_eq!(m.identity.unwrap().instance(), "main");

        // An empty instance token falls back to the default.
        let m = MessageBuilder::new("N", "1.0").from_config(&cfg).instance("").build();
        assert_eq!(m.identity.unwrap().instance(), "main");
    }

    #[test]
    fn explicit_tags_win_over_config_tags() {
        let cfg = Config::from_value("c", "t", json!({ "tags": { "site": "f1" } })).unwrap();
        let m = MessageBuilder::new("N", "1.0")
            .tag("site", json!("override"))
            .tag("extra", json!(1))
            .from_config(&cfg)
            .build();
        let tags = m.tags.unwrap();
        assert_eq!(tags.extra.get("site"), Some(&json!("override")));
        assert_eq!(tags.extra.get("extra"), Some(&json!(1)));
    }
}
