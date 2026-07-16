//! # Messaging — Message model
//!
//! **One-liner purpose**: The `Message` value type (header + identity + tags + body)
//! and its fluent [`MessageBuilder`], plus protobuf serialization for the wire.
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
//! per-message `instance` (`None` ⇒ component scope, D-U28). It is **optional on the wire**: a
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
//! use edgecommons::messaging::message::MessageBuilder;
//! use serde_json::json;
//!
//! let msg = MessageBuilder::new("ProcessData", "1.0")
//!     .payload(json!({ "value": 42 }))
//!     .build();
//! assert_eq!(msg.header.name, "ProcessData");
//! let bytes = msg.to_vec().unwrap();
//! let round_tripped = edgecommons::messaging::message::Message::from_slice(&bytes).unwrap();
//! assert_eq!(round_tripped.header.name, "ProcessData");
//! ```
//!
//! ## Related Modules
//! - [`crate::messaging::service`] — uses messages for publish / request / reply.
//! - [`crate::uns`] — builds the topics these envelopes are published on.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use prost::Message as ProstMessage;
use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::config::model::Config;
use crate::error::{EdgeCommonsError, Result};
use crate::proto::edgecommons::v1 as pb;

/// Maximum decoded size for the first-class binary message body marker.
pub const MAX_BINARY_BODY_BYTES: usize = 64 * 1024;
const BINARY_BODY_KEY: &str = "_edgecommonsBinary";
const BINARY_ENCODING: &str = "base64";
const DATA_MESSAGE_NAME: &str = "SouthboundSignalUpdate";
const TELEMETRY_MESSAGE_NAME: &str = "Telemetry";
const DEFAULT_OPAQUE_CONTENT_TYPE: &str = "application/octet-stream";

fn binary_body_value(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_BINARY_BODY_BYTES {
        return Err(EdgeCommonsError::Messaging(format!(
            "Binary message body exceeds {MAX_BINARY_BODY_BYTES} bytes"
        )));
    }
    let mut descriptor = Map::new();
    descriptor.insert(
        "encoding".to_string(),
        Value::String(BINARY_ENCODING.to_string()),
    );
    descriptor.insert("length".to_string(), Value::Number(bytes.len().into()));
    descriptor.insert(
        "data".to_string(),
        Value::String(BASE64_STANDARD.encode(bytes)),
    );
    let mut marker = Map::new();
    marker.insert(BINARY_BODY_KEY.to_string(), Value::Object(descriptor));
    Ok(Value::Object(marker))
}

/// Build the first-class binary marker used for byte-valued structured data.
///
/// This is useful for sample values and nested structured fields that must encode
/// as `EcValue.bytes_value` while still living inside the JSON-shaped public body.
pub fn binary_value(bytes: impl AsRef<[u8]>) -> Result<Value> {
    binary_body_value(bytes.as_ref())
}

fn has_binary_marker(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|obj| obj.contains_key(BINARY_BODY_KEY))
}

fn binary_descriptor(value: &Value) -> Result<Option<&Map<String, Value>>> {
    let Some(obj) = value.as_object() else {
        return Ok(None);
    };
    let Some(descriptor) = obj.get(BINARY_BODY_KEY) else {
        return Ok(None);
    };
    descriptor.as_object().map(Some).ok_or_else(|| {
        EdgeCommonsError::Messaging("Binary message body marker must be an object".to_string())
    })
}

fn decode_binary_descriptor(descriptor: &Map<String, Value>) -> Result<Vec<u8>> {
    let encoding = descriptor
        .get("encoding")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            EdgeCommonsError::Messaging("Binary message body encoding must be base64".to_string())
        })?;
    if encoding != BINARY_ENCODING {
        return Err(EdgeCommonsError::Messaging(
            "Binary message body encoding must be base64".to_string(),
        ));
    }
    let declared_length = descriptor
        .get("length")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "Binary message body length must be a non-negative integer".to_string(),
            )
        })?;
    let declared_length = usize::try_from(declared_length).map_err(|_| {
        EdgeCommonsError::Messaging(format!(
            "Binary message body exceeds {MAX_BINARY_BODY_BYTES} bytes"
        ))
    })?;
    if declared_length > MAX_BINARY_BODY_BYTES {
        return Err(EdgeCommonsError::Messaging(format!(
            "Binary message body exceeds {MAX_BINARY_BODY_BYTES} bytes"
        )));
    }
    let encoded = descriptor
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            EdgeCommonsError::Messaging("Binary message body data is required".to_string())
        })?;
    let decoded = BASE64_STANDARD.decode(encoded).map_err(|_| {
        EdgeCommonsError::Messaging("Binary message body data is not valid base64".to_string())
    })?;
    if decoded.len() != declared_length {
        return Err(EdgeCommonsError::Messaging(
            "Binary message body length does not match decoded data".to_string(),
        ));
    }
    Ok(decoded)
}

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
    /// UTC creation timestamp as epoch milliseconds, the canonical protobuf wire value.
    #[serde(default)]
    pub timestamp_ms: u64,
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
/// - `instance` — the per-message instance token, or `None` for component/global
///   scope (D-U28); never a reserved UNS class token.
///
/// Serialization emits the canonical member order `hier, path, component, instance`
/// (field order = emit order; `instance` is **omitted** when `None` — D-U28).
/// Deserialization ([`Self::from_wire`]) is deliberately lenient: a malformed
/// identity yields `None` plus a WARN log and the enclosing message still delivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageIdentity {
    hier: Vec<HierEntry>,
    path: String,
    component: String,
    /// D-U28: `None` ⇒ component/global scope (the `instance` wire key is omitted);
    /// a present token is never a reserved UNS class token.
    #[serde(skip_serializing_if = "Option::is_none")]
    instance: Option<String>,
}

impl MessageIdentity {
    /// Creates a validated identity, precomputing `path` as the `'/'`-join of the
    /// `hier` values. An absent/empty `instance` means component/global scope
    /// (D-U28: the identity carries no `instance`).
    ///
    /// # Errors
    /// [`EdgeCommonsError::Messaging`] when `hier` is empty, an entry's level/value is empty,
    /// `component` is empty, or a present `instance` equals a reserved UNS class token.
    pub fn new(
        hier: Vec<HierEntry>,
        component: impl Into<String>,
        instance: Option<String>,
    ) -> Result<MessageIdentity> {
        let component = component.into();
        if hier.is_empty() {
            return Err(EdgeCommonsError::Messaging(
                "MessageIdentity hier must contain at least one entry".to_string(),
            ));
        }
        for entry in &hier {
            if entry.level.is_empty() {
                return Err(EdgeCommonsError::Messaging(
                    "MessageIdentity hier entry level must be non-empty".to_string(),
                ));
            }
            if entry.value.is_empty() {
                return Err(EdgeCommonsError::Messaging(format!(
                    "MessageIdentity hier entry value for level '{}' must be non-empty",
                    entry.level
                )));
            }
        }
        if component.is_empty() {
            return Err(EdgeCommonsError::Messaging(
                "MessageIdentity component must be non-empty".to_string(),
            ));
        }
        let path = hier
            .iter()
            .map(|e| e.value.as_str())
            .collect::<Vec<_>>()
            .join("/");
        let instance = normalize_instance(instance.as_deref())?;
        Ok(MessageIdentity {
            hier,
            path,
            component,
            instance,
        })
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

    /// Returns the per-message instance token, or `None` for component/global scope
    /// (D-U28).
    pub fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// Computed accessor — the last `hier` entry's value. NOT a wire field: the
    /// device is inherent to the hierarchy (its deepest level), so it is never
    /// serialized separately.
    pub fn device(&self) -> &str {
        &self.hier[self.hier.len() - 1].value
    }

    /// Returns a copy of this identity with a different per-message instance token,
    /// or component/global scope when `instance` is empty (D-U28).
    ///
    /// # Errors
    /// [`EdgeCommonsError::Messaging`] when a present `instance` equals a reserved UNS class token.
    pub fn with_instance(&self, instance: impl Into<String>) -> Result<MessageIdentity> {
        let instance = instance.into();
        Ok(MessageIdentity {
            hier: self.hier.clone(),
            path: self.path.clone(),
            component: self.component.clone(),
            instance: normalize_instance(Some(&instance))?,
        })
    }

    /// Infallible internal variant of [`Self::with_instance`] for the builder's
    /// stamping site: an empty/absent token means component scope (D-U28: `None`).
    /// A present token is not re-validated here (the builder is app-driven and
    /// `build()` is infallible); validation happens at the [`Self::new`] /
    /// [`Self::with_instance`] construction sites.
    pub(crate) fn with_instance_infallible(&self, instance: Option<&str>) -> MessageIdentity {
        let instance = instance.filter(|s| !s.is_empty()).map(str::to_string);
        MessageIdentity {
            hier: self.hier.clone(),
            path: self.path.clone(),
            component: self.component.clone(),
            instance,
        }
    }

    /// Lenient wire-form parser (mirrors Java `MessageIdentity.fromDict`): a missing/empty
    /// `instance` means component scope (`None`, D-U28); a missing `path` is
    /// recomputed from the hier values (a present one is taken as-is — the publisher
    /// is authoritative); a malformed identity (non-object element,
    /// missing/empty/non-array `hier`, malformed hier entries, a missing
    /// `component`, or an `instance` equal to a reserved class token) yields `None`
    /// plus a WARN log so the enclosing message still
    /// delivers.
    pub fn from_wire(src: &Value) -> Option<MessageIdentity> {
        let Some(obj) = src.as_object() else {
            tracing::warn!(
                "Malformed message identity: 'identity' is not an object; dropping identity"
            );
            return None;
        };
        let Some(hier_arr) = obj
            .get("hier")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
        else {
            tracing::warn!(
                "Malformed message identity: 'hier' missing, not an array, or empty; dropping identity"
            );
            return None;
        };
        let mut hier = Vec::with_capacity(hier_arr.len());
        for entry in hier_arr {
            let Some(entry_obj) = entry.as_object() else {
                tracing::warn!(
                    "Malformed message identity: hier entry is not an object; dropping identity"
                );
                return None;
            };
            let level = non_empty_str(entry_obj.get("level"));
            let value = non_empty_str(entry_obj.get("value"));
            let (Some(level), Some(value)) = (level, value) else {
                tracing::warn!(
                    "Malformed message identity: hier entry missing level/value; dropping identity"
                );
                return None;
            };
            hier.push(HierEntry {
                level: level.to_string(),
                value: value.to_string(),
            });
        }
        let Some(component) = non_empty_str(obj.get("component")) else {
            tracing::warn!(
                "Malformed message identity: 'component' missing or empty; dropping identity"
            );
            return None;
        };
        let path = match non_empty_str(obj.get("path")) {
            Some(p) => p.to_string(), // present => taken as-is (publisher is authoritative)
            None => hier
                .iter()
                .map(|e| e.value.as_str())
                .collect::<Vec<_>>()
                .join("/"),
        };
        // D-U28: a missing/empty instance means component scope; a present instance
        // that is a reserved class token makes the identity malformed (dropped
        // leniently, mirroring the Java canonical routing through the constructor).
        let instance = match non_empty_str(obj.get("instance")) {
            None => None,
            Some(token) => match normalize_instance(Some(token)) {
                Ok(instance) => instance,
                Err(_) => {
                    tracing::warn!(
                        instance = token,
                        "Malformed message identity: 'instance' is a reserved UNS class token; \
                         dropping identity"
                    );
                    return None;
                }
            },
        };
        Some(MessageIdentity {
            hier,
            path,
            component: component.to_string(),
            instance,
        })
    }
}

/// D-U28 instance normalization: an empty/absent token means component/global scope
/// (`None`); a present token may not equal a reserved UNS class token
/// (`state`/`metric`/`cfg`/`log`/`data`/`evt`/`cmd`/`app`), which would collapse the
/// component-scope and instance-scope UNS templates and defeat the reserved-class guard.
///
/// # Errors
/// [`EdgeCommonsError::Messaging`] when a present token is a reserved UNS class token.
fn normalize_instance(instance: Option<&str>) -> Result<Option<String>> {
    match instance.filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(token) => {
            if crate::uns::UnsClass::from_token(token).is_some() {
                return Err(EdgeCommonsError::Messaging(format!(
                    "MessageIdentity instance '{token}' must not be a reserved UNS class token"
                )));
            }
            Ok(Some(token.to_string()))
        }
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

/// The selected protobuf body lane for an EdgeCommons message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageBodyCase {
    SouthboundSignalUpdate,
    StateUpdate,
    ConfigUpdate,
    MetricUpdate,
    Event,
    Command,
    Structured,
    Opaque,
    #[default]
    BodyNotSet,
}

impl MessageBodyCase {
    /// Java-compatible diagnostic spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SouthboundSignalUpdate => "SOUTHBOUND_SIGNAL_UPDATE",
            Self::StateUpdate => "STATE_UPDATE",
            Self::ConfigUpdate => "CONFIG_UPDATE",
            Self::MetricUpdate => "METRIC_UPDATE",
            Self::Event => "EVENT",
            Self::Command => "COMMAND",
            Self::Structured => "STRUCTURED",
            Self::Opaque => "OPAQUE",
            Self::BodyNotSet => "BODY_NOT_SET",
        }
    }
}

/// Optional schema metadata for an opaque or structured body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageBodySchema {
    pub name: Option<String>,
    pub version: Option<String>,
    pub content_type: Option<String>,
    pub descriptor_ref: Option<String>,
    pub hash: Option<String>,
}

impl MessageBodySchema {
    fn to_value(&self) -> Value {
        let mut obj = Map::new();
        if let Some(name) = &self.name {
            obj.insert("name".to_string(), Value::String(name.clone()));
        }
        if let Some(version) = &self.version {
            obj.insert("version".to_string(), Value::String(version.clone()));
        }
        if let Some(content_type) = &self.content_type {
            obj.insert(
                "content_type".to_string(),
                Value::String(content_type.clone()),
            );
        }
        if let Some(descriptor_ref) = &self.descriptor_ref {
            obj.insert(
                "descriptor_ref".to_string(),
                Value::String(descriptor_ref.clone()),
            );
        }
        if let Some(hash) = &self.hash {
            obj.insert("hash".to_string(), Value::String(hash.clone()));
        }
        Value::Object(obj)
    }

    fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        Some(Self {
            name: optional_string(obj.get("name")),
            version: optional_string(obj.get("version")),
            content_type: optional_string(obj.get("content_type")),
            descriptor_ref: optional_string(obj.get("descriptor_ref")),
            hash: optional_string(obj.get("hash")),
        })
    }
}

fn optional_string(element: Option<&Value>) -> Option<String> {
    element.and_then(Value::as_str).map(ToString::to_string)
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
    /// Optional body content type. Opaque protobuf bodies default to
    /// `application/octet-stream` when absent.
    pub content_type: Option<String>,
    /// Optional body content encoding.
    pub content_encoding: Option<String>,
    /// Optional body schema metadata.
    pub schema: Option<MessageBodySchema>,
    /// Explicit body lane. [`MessageBodyCase::BodyNotSet`] lets the codec infer the
    /// Java-compatible default from header/body.
    pub body_case: MessageBodyCase,
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
            let entries = 2
                + usize::from(self.identity.is_some())
                + usize::from(self.tags.is_some())
                + usize::from(self.content_type.is_some())
                + usize::from(self.content_encoding.is_some())
                + usize::from(self.schema.is_some());
            let mut map = serializer.serialize_map(Some(entries))?;
            map.serialize_entry("header", &self.header)?;
            // Canonical envelope member order: header, identity, tags, body.
            if let Some(identity) = &self.identity {
                map.serialize_entry("identity", identity)?;
            }
            if let Some(tags) = &self.tags {
                map.serialize_entry("tags", tags)?;
            }
            if let Some(content_type) = &self.content_type {
                map.serialize_entry("content_type", content_type)?;
            }
            if let Some(content_encoding) = &self.content_encoding {
                map.serialize_entry("content_encoding", content_encoding)?;
            }
            if let Some(schema) = &self.schema {
                map.serialize_entry("schema", &schema.to_value())?;
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
            content_type: None,
            content_encoding: None,
            schema: None,
            body_case: MessageBodyCase::BodyNotSet,
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

    /// Whether the payload is a first-class binary body marker.
    pub fn is_binary_body(&self) -> bool {
        has_binary_marker(&self.body)
    }

    /// Decode the first-class binary message body.
    ///
    /// Returns `Ok(None)` when the body is not binary. Returns an error when the
    /// inbound binary marker is malformed or exceeds [`MAX_BINARY_BODY_BYTES`].
    pub fn binary_body(&self) -> Result<Option<Vec<u8>>> {
        let Some(descriptor) = binary_descriptor(&self.body)? else {
            return Ok(None);
        };
        decode_binary_descriptor(descriptor).map(Some)
    }

    /// The effective body case, inferred the same way as Java when not pinned.
    pub fn body_case(&self) -> MessageBodyCase {
        if self.body_case != MessageBodyCase::BodyNotSet {
            return self.body_case;
        }
        if self.body.is_null() {
            return MessageBodyCase::BodyNotSet;
        }
        if self.is_binary_body() {
            return MessageBodyCase::Opaque;
        }
        if (self.header.name == DATA_MESSAGE_NAME || self.header.name == TELEMETRY_MESSAGE_NAME)
            && self.body.is_object()
        {
            return MessageBodyCase::SouthboundSignalUpdate;
        }
        if self.body.is_object() {
            match self.header.name.as_str() {
                name if name.eq_ignore_ascii_case("state") => return MessageBodyCase::StateUpdate,
                name if name.eq_ignore_ascii_case("cfg") => return MessageBodyCase::ConfigUpdate,
                "Config" | "Configuration" => return MessageBodyCase::ConfigUpdate,
                "Metric" | "metric" => return MessageBodyCase::MetricUpdate,
                name if name.eq_ignore_ascii_case("evt") => return MessageBodyCase::Event,
                "Event" => return MessageBodyCase::Event,
                _ => {}
            }
        }
        MessageBodyCase::Structured
    }

    /// Decodes the opaque body bytes when this message uses the opaque body lane.
    pub fn opaque_body(&self) -> Result<Option<Vec<u8>>> {
        if self.body_case() == MessageBodyCase::Opaque {
            self.binary_body()
        } else {
            Ok(None)
        }
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
                let content_type = optional_string(map.get("content_type"));
                let content_encoding = optional_string(map.get("content_encoding"));
                let schema = map.get("schema").and_then(MessageBodySchema::from_value);
                let body = map.get("body").cloned().unwrap_or(Value::Null);
                return Ok(Message {
                    header,
                    identity,
                    tags,
                    content_type,
                    content_encoding,
                    schema,
                    body_case: MessageBodyCase::BodyNotSet,
                    body,
                    raw: None,
                });
            }
        }
        // Non-envelope (or non-object): deliver as raw, matching Java/Python.
        Ok(Message::raw(value))
    }

    /// Serialize this message to protobuf bytes for the wire.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `EdgeCommonsError::Messaging` | The message is raw or missing required protobuf envelope fields | Publish foreign payloads through `publish_raw` |
    pub fn to_vec(&self) -> Result<Vec<u8>> {
        let proto = self.to_proto()?;
        Ok(proto.encode_to_vec())
    }

    /// Deserialize an EdgeCommons protobuf message from bytes received off the wire.
    ///
    /// Raw, foreign, JSON, and malformed payloads are rejected so normal Message
    /// subscriptions can log/drop them instead of delivering synthetic raw messages.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `EdgeCommonsError::Messaging` | Bytes are not a valid EdgeCommons protobuf envelope | Use raw transport handling for non-EdgeCommons payloads |
    pub fn from_slice(bytes: &[u8]) -> Result<Message> {
        let proto = pb::EdgeCommonsMessage::decode(bytes).map_err(|e| {
            EdgeCommonsError::Messaging(format!("Malformed EdgeCommons protobuf message: {e}"))
        })?;
        Message::from_proto(proto)
    }

    fn to_proto(&self) -> Result<pb::EdgeCommonsMessage> {
        if self.raw.is_some() {
            return Err(EdgeCommonsError::Messaging(
                "EdgeCommons protobuf message requires an envelope; use publish_raw for raw payloads"
                    .to_string(),
            ));
        }
        if self.header.name.is_empty() || self.header.version.is_empty() {
            return Err(EdgeCommonsError::Messaging(
                "EdgeCommons protobuf message requires header name and version".to_string(),
            ));
        }

        let mut msg = pb::EdgeCommonsMessage {
            header: Some(pb::Header {
                name: self.header.name.clone(),
                version: self.header.version.clone(),
                timestamp_ms: self.header.timestamp_ms,
                uuid: self.header.uuid.clone(),
                correlation_id: Some(self.header.correlation_id.clone()),
                reply_to: self.header.reply_to.clone(),
            }),
            identity: self.identity.as_ref().map(to_proto_identity),
            tags: self
                .tags
                .as_ref()
                .map(|tags| {
                    tags.extra
                        .iter()
                        .map(|(key, value)| Ok((key.clone(), to_ec_value(value)?)))
                        .collect::<Result<_>>()
                })
                .transpose()?
                .unwrap_or_default(),
            content_type: self.content_type.clone().unwrap_or_default(),
            content_encoding: self.content_encoding.clone().unwrap_or_default(),
            schema: self.schema.as_ref().map(to_proto_schema),
            body: None,
        };

        msg.body = match self.body_case() {
            MessageBodyCase::Opaque => {
                let body = self.binary_body()?.unwrap_or_default();
                if msg.content_type.is_empty() {
                    msg.content_type = DEFAULT_OPAQUE_CONTENT_TYPE.to_string();
                }
                Some(pb::edge_commons_message::Body::Opaque(body))
            }
            MessageBodyCase::SouthboundSignalUpdate => Some(
                pb::edge_commons_message::Body::SouthboundSignalUpdate(to_telemetry(&self.body)?),
            ),
            MessageBodyCase::StateUpdate => Some(pb::edge_commons_message::Body::StateUpdate(
                to_state_update(&self.body)?,
            )),
            MessageBodyCase::ConfigUpdate => Some(pb::edge_commons_message::Body::ConfigUpdate(
                to_config_update(&self.body)?,
            )),
            MessageBodyCase::MetricUpdate => Some(pb::edge_commons_message::Body::MetricUpdate(
                to_metric_update(&self.body)?,
            )),
            MessageBodyCase::Event => {
                Some(pb::edge_commons_message::Body::Event(to_event(&self.body)?))
            }
            MessageBodyCase::Command => Some(pb::edge_commons_message::Body::Command(to_command(
                &self.header.name,
                &self.body,
            )?)),
            MessageBodyCase::Structured => Some(pb::edge_commons_message::Body::Structured(
                to_ec_value(&self.body)?,
            )),
            MessageBodyCase::BodyNotSet => None,
        };

        Ok(msg)
    }

    fn from_proto(proto: pb::EdgeCommonsMessage) -> Result<Message> {
        let header = proto.header.ok_or_else(|| {
            EdgeCommonsError::Messaging(
                "EdgeCommons protobuf message requires header name and version".to_string(),
            )
        })?;
        if header.name.is_empty() || header.version.is_empty() {
            return Err(EdgeCommonsError::Messaging(
                "EdgeCommons protobuf message requires header name and version".to_string(),
            ));
        }

        let mut builder = MessageBuilder::new(header.name, header.version)
            .timestamp_ms(header.timestamp_ms)
            .uuid(header.uuid);
        if let Some(correlation_id) = header.correlation_id {
            builder = builder.correlation_id(correlation_id);
        }
        if let Some(reply_to) = header.reply_to {
            builder = builder.reply_to(reply_to);
        }
        if let Some(identity) = proto.identity {
            builder = builder.identity(from_proto_identity(identity)?);
        }
        if !proto.tags.is_empty() {
            builder = builder.tags(MessageTags {
                extra: proto
                    .tags
                    .into_iter()
                    .map(|(key, value)| (key, from_ec_value(value)))
                    .collect(),
            });
        }
        if !proto.content_type.is_empty() {
            builder = builder.content_type(proto.content_type);
        }
        if !proto.content_encoding.is_empty() {
            builder = builder.content_encoding(proto.content_encoding);
        }
        if let Some(schema) = proto.schema {
            builder = builder.schema(from_proto_schema(schema));
        }

        builder = match proto.body {
            Some(pb::edge_commons_message::Body::SouthboundSignalUpdate(body)) => {
                builder.southbound_signal_update(from_telemetry(body))
            }
            Some(pb::edge_commons_message::Body::StateUpdate(body)) => builder
                .payload(from_state_update(body))
                .body_case(MessageBodyCase::StateUpdate),
            Some(pb::edge_commons_message::Body::ConfigUpdate(body)) => builder
                .payload(from_config_update(body))
                .body_case(MessageBodyCase::ConfigUpdate),
            Some(pb::edge_commons_message::Body::MetricUpdate(body)) => builder
                .payload(from_metric_update(body))
                .body_case(MessageBodyCase::MetricUpdate),
            Some(pb::edge_commons_message::Body::Event(body)) => builder
                .payload(from_event(body))
                .body_case(MessageBodyCase::Event),
            Some(pb::edge_commons_message::Body::Command(body)) => builder
                .payload(from_command(body))
                .body_case(MessageBodyCase::Command),
            Some(pb::edge_commons_message::Body::Structured(value)) => {
                builder.structured_payload(from_ec_value(value))
            }
            Some(pb::edge_commons_message::Body::Opaque(bytes)) => {
                let content_type = builder
                    .content_type
                    .clone()
                    .unwrap_or_else(|| DEFAULT_OPAQUE_CONTENT_TYPE.to_string());
                builder.opaque_payload(bytes, content_type)?
            }
            None => builder.body_case(MessageBodyCase::BodyNotSet),
        };
        Ok(builder.build())
    }

    /// The correlation id of this message.
    pub fn correlation_id(&self) -> &str {
        &self.header.correlation_id
    }
}

fn to_proto_identity(identity: &MessageIdentity) -> pb::Identity {
    pb::Identity {
        hier: identity
            .hier()
            .iter()
            .map(|entry| pb::HierEntry {
                level: entry.level.clone(),
                value: entry.value.clone(),
            })
            .collect(),
        path: identity.path().to_string(),
        component: identity.component().to_string(),
        // D-U28: component scope omits the instance; proto3's empty-string default
        // IS the "absent" wire form (parsed back to None by from_proto_identity).
        instance: identity.instance().unwrap_or_default().to_string(),
    }
}

fn from_proto_identity(identity: pb::Identity) -> Result<MessageIdentity> {
    let hier: Vec<HierEntry> = identity
        .hier
        .into_iter()
        .map(|entry| HierEntry {
            level: entry.level,
            value: entry.value,
        })
        .collect();
    let mut value = Map::new();
    value.insert("hier".to_string(), serde_json::to_value(hier)?);
    value.insert("path".to_string(), Value::String(identity.path));
    value.insert("component".to_string(), Value::String(identity.component));
    value.insert("instance".to_string(), Value::String(identity.instance));
    MessageIdentity::from_wire(&Value::Object(value))
        .ok_or_else(|| EdgeCommonsError::Messaging("Malformed protobuf identity".to_string()))
}

fn to_proto_schema(schema: &MessageBodySchema) -> pb::BodySchema {
    pb::BodySchema {
        name: schema.name.clone().unwrap_or_default(),
        version: schema.version.clone().unwrap_or_default(),
        content_type: schema.content_type.clone().unwrap_or_default(),
        descriptor_ref: schema.descriptor_ref.clone().unwrap_or_default(),
        hash: schema.hash.clone().unwrap_or_default(),
    }
}

fn from_proto_schema(schema: pb::BodySchema) -> MessageBodySchema {
    MessageBodySchema {
        name: none_if_empty(schema.name),
        version: none_if_empty(schema.version),
        content_type: none_if_empty(schema.content_type),
        descriptor_ref: none_if_empty(schema.descriptor_ref),
        hash: none_if_empty(schema.hash),
    }
}

fn none_if_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn to_ec_value(value: &Value) -> Result<pb::EcValue> {
    let kind = match value {
        Value::Null => pb::ec_value::Kind::NullValue(0),
        Value::Bool(v) => pb::ec_value::Kind::BoolValue(*v),
        Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                pb::ec_value::Kind::IntValue(v)
            } else if let Some(v) = n.as_u64() {
                pb::ec_value::Kind::UintValue(v)
            } else {
                let v = n.as_f64().ok_or_else(|| {
                    EdgeCommonsError::Messaging(
                        "EdgeCommons protobuf structured values reject NaN and infinity"
                            .to_string(),
                    )
                })?;
                if !v.is_finite() {
                    return Err(EdgeCommonsError::Messaging(
                        "EdgeCommons protobuf structured values reject NaN and infinity"
                            .to_string(),
                    ));
                }
                pb::ec_value::Kind::DoubleValue(v)
            }
        }
        Value::String(v) => pb::ec_value::Kind::StringValue(v.clone()),
        Value::Array(values) => pb::ec_value::Kind::ListValue(pb::EcList {
            values: values.iter().map(to_ec_value).collect::<Result<_>>()?,
        }),
        Value::Object(obj) => {
            if let Some(descriptor) = binary_descriptor(value)? {
                pb::ec_value::Kind::BytesValue(decode_binary_descriptor(descriptor)?)
            } else {
                pb::ec_value::Kind::MapValue(pb::EcMap {
                    fields: obj
                        .iter()
                        .map(|(key, value)| Ok((key.clone(), to_ec_value(value)?)))
                        .collect::<Result<_>>()?,
                })
            }
        }
    };
    Ok(pb::EcValue { kind: Some(kind) })
}

fn from_ec_value(value: pb::EcValue) -> Value {
    match value.kind {
        Some(pb::ec_value::Kind::NullValue(_)) | None => Value::Null,
        Some(pb::ec_value::Kind::BoolValue(v)) => Value::Bool(v),
        Some(pb::ec_value::Kind::IntValue(v)) => Value::Number(v.into()),
        Some(pb::ec_value::Kind::UintValue(v)) => Value::Number(serde_json::Number::from(v)),
        Some(pb::ec_value::Kind::DoubleValue(v)) => serde_json::Number::from_f64(v)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(pb::ec_value::Kind::StringValue(v)) => Value::String(v),
        Some(pb::ec_value::Kind::BytesValue(v)) => binary_body_value(&v).unwrap_or(Value::Null),
        Some(pb::ec_value::Kind::ListValue(list)) => {
            Value::Array(list.values.into_iter().map(from_ec_value).collect())
        }
        Some(pb::ec_value::Kind::MapValue(map)) => Value::Object(
            map.fields
                .into_iter()
                .map(|(key, value)| (key, from_ec_value(value)))
                .collect(),
        ),
    }
}

fn to_telemetry(body: &Value) -> Result<pb::SouthboundSignalUpdate> {
    let obj = body.as_object().ok_or_else(|| {
        EdgeCommonsError::Messaging("SouthboundSignalUpdate body must be an object".to_string())
    })?;
    let signal = obj
        .get("signal")
        .and_then(Value::as_object)
        .map(to_signal)
        .transpose()?;
    let samples = obj
        .get("samples")
        .and_then(Value::as_array)
        .map(|samples| {
            samples
                .iter()
                .filter_map(Value::as_object)
                .map(to_sample)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(pb::SouthboundSignalUpdate {
        signal,
        samples,
        extra: copy_extra(obj, &["signal", "samples"])?,
    })
}

fn to_signal(obj: &Map<String, Value>) -> Result<pb::Signal> {
    Ok(pb::Signal {
        id: str_value(obj, "id").unwrap_or_default(),
        name: str_value(obj, "name").unwrap_or_default(),
        address: obj.get("address").map(to_ec_value).transpose()?,
        extra: copy_extra(obj, &["id", "name", "address"])?,
    })
}

fn to_sample(obj: &Map<String, Value>) -> Result<pb::Sample> {
    let source_ts = str_value(obj, "sourceTs").or_else(|| str_value(obj, "source_ts"));
    let server_ts = str_value(obj, "serverTs").or_else(|| str_value(obj, "server_ts"));
    let source_ts_ms = u64_value(obj, "sourceTsMs")
        .or_else(|| u64_value(obj, "source_ts_ms"))
        .or_else(|| {
            source_ts
                .as_deref()
                .map(timestamp_ms_from_rfc3339)
                .filter(|v| *v != 0)
        });
    let server_ts_ms = u64_value(obj, "serverTsMs")
        .or_else(|| u64_value(obj, "server_ts_ms"))
        .or_else(|| {
            server_ts
                .as_deref()
                .map(timestamp_ms_from_rfc3339)
                .filter(|v| *v != 0)
        });
    Ok(pb::Sample {
        value: obj.get("value").map(to_ec_value).transpose()?,
        quality: str_value(obj, "quality").unwrap_or_default(),
        quality_raw: obj
            .get("qualityRaw")
            .or_else(|| obj.get("quality_raw"))
            .map(to_ec_value)
            .transpose()?,
        source_ts,
        source_ts_ms,
        server_ts,
        server_ts_ms,
        extra: copy_extra(
            obj,
            &[
                "value",
                "quality",
                "qualityRaw",
                "quality_raw",
                "sourceTs",
                "source_ts",
                "sourceTsMs",
                "source_ts_ms",
                "serverTs",
                "server_ts",
                "serverTsMs",
                "server_ts_ms",
            ],
        )?,
    })
}

fn from_telemetry(telemetry: pb::SouthboundSignalUpdate) -> Value {
    let mut obj = Map::new();
    if let Some(signal) = telemetry.signal {
        let mut signal_obj = Map::new();
        signal_obj.insert("id".to_string(), Value::String(signal.id));
        if !signal.name.is_empty() {
            signal_obj.insert("name".to_string(), Value::String(signal.name));
        }
        if let Some(address) = signal.address {
            signal_obj.insert("address".to_string(), from_ec_value(address));
        }
        extend_extra(&mut signal_obj, signal.extra);
        obj.insert("signal".to_string(), Value::Object(signal_obj));
    }
    obj.insert(
        "samples".to_string(),
        Value::Array(telemetry.samples.into_iter().map(from_sample).collect()),
    );
    extend_extra(&mut obj, telemetry.extra);
    Value::Object(obj)
}

fn from_sample(sample: pb::Sample) -> Value {
    let mut obj = Map::new();
    if let Some(value) = sample.value {
        obj.insert("value".to_string(), from_ec_value(value));
    }
    if !sample.quality.is_empty() {
        obj.insert("quality".to_string(), Value::String(sample.quality));
    }
    if let Some(quality_raw) = sample.quality_raw {
        obj.insert("qualityRaw".to_string(), from_ec_value(quality_raw));
    }
    if let Some(source_ts) = sample.source_ts {
        obj.insert("sourceTs".to_string(), Value::String(source_ts));
    }
    if let Some(source_ts_ms) = sample.source_ts_ms {
        obj.insert("sourceTsMs".to_string(), Value::Number(source_ts_ms.into()));
    }
    if let Some(server_ts) = sample.server_ts {
        obj.insert("serverTs".to_string(), Value::String(server_ts));
    }
    if let Some(server_ts_ms) = sample.server_ts_ms {
        obj.insert("serverTsMs".to_string(), Value::Number(server_ts_ms.into()));
    }
    extend_extra(&mut obj, sample.extra);
    Value::Object(obj)
}

fn to_state_update(body: &Value) -> Result<pb::StateUpdate> {
    let obj = as_object(body, "StateUpdate")?;
    Ok(pb::StateUpdate {
        status: str_value(obj, "status").unwrap_or_default(),
        uptime_secs: u64_value(obj, "uptimeSecs").or_else(|| u64_value(obj, "uptime_secs")),
        instances: obj
            .get("instances")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_object)
                    .map(to_instance_connectivity)
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default(),
        extra: copy_extra(obj, &["status", "uptimeSecs", "uptime_secs", "instances"])?,
    })
}

fn to_instance_connectivity(obj: &Map<String, Value>) -> Result<pb::InstanceConnectivity> {
    Ok(pb::InstanceConnectivity {
        instance: str_value(obj, "instance").unwrap_or_default(),
        connected: obj
            .get("connected")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        detail: str_value(obj, "detail"),
        extra: copy_extra(obj, &["instance", "connected", "detail"])?,
    })
}

fn from_state_update(state: pb::StateUpdate) -> Value {
    let mut obj = Map::new();
    if !state.status.is_empty() {
        obj.insert("status".to_string(), Value::String(state.status));
    }
    if let Some(uptime_secs) = state.uptime_secs {
        obj.insert("uptimeSecs".to_string(), Value::Number(uptime_secs.into()));
    }
    if !state.instances.is_empty() {
        obj.insert(
            "instances".to_string(),
            Value::Array(
                state
                    .instances
                    .into_iter()
                    .map(|item| {
                        let mut obj = Map::new();
                        obj.insert("instance".to_string(), Value::String(item.instance));
                        obj.insert("connected".to_string(), Value::Bool(item.connected));
                        if let Some(detail) = item.detail {
                            obj.insert("detail".to_string(), Value::String(detail));
                        }
                        extend_extra(&mut obj, item.extra);
                        Value::Object(obj)
                    })
                    .collect(),
            ),
        );
    }
    extend_extra(&mut obj, state.extra);
    Value::Object(obj)
}

fn to_config_update(body: &Value) -> Result<pb::ConfigUpdate> {
    let obj = as_object(body, "ConfigUpdate")?;
    Ok(pb::ConfigUpdate {
        config: obj.get("config").map(to_ec_value).transpose()?,
        extra: copy_extra(obj, &["config"])?,
    })
}

fn from_config_update(config: pb::ConfigUpdate) -> Value {
    let mut obj = Map::new();
    if let Some(config) = config.config {
        obj.insert("config".to_string(), from_ec_value(config));
    }
    extend_extra(&mut obj, config.extra);
    Value::Object(obj)
}

fn to_metric_update(body: &Value) -> Result<pb::MetricUpdate> {
    let obj = as_object(body, "MetricUpdate")?;
    let dimensions = obj
        .get("dimensions")
        .and_then(Value::as_object)
        .map(|dims| {
            dims.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let values = obj
        .get("values")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_object)
                .map(|item| pb::MetricValue {
                    name: str_value(item, "name").unwrap_or_default(),
                    value: f64_value(item, "value").unwrap_or_default(),
                    unit: str_value(item, "unit").unwrap_or_default(),
                    storage_resolution: u64_value(item, "storageResolution")
                        .or_else(|| u64_value(item, "storage_resolution"))
                        .unwrap_or_default() as u32,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(pb::MetricUpdate {
        namespace: str_value(obj, "namespace").unwrap_or_default(),
        metric_name: str_value(obj, "metricName")
            .or_else(|| str_value(obj, "metric_name"))
            .unwrap_or_default(),
        timestamp_ms: u64_value(obj, "timestampMs")
            .or_else(|| u64_value(obj, "timestamp_ms"))
            .unwrap_or_default(),
        dimensions,
        values,
        large_fleet_workaround: obj
            .get("largeFleetWorkaround")
            .or_else(|| obj.get("large_fleet_workaround"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        emf_projection: obj
            .get("emfProjection")
            .or_else(|| obj.get("emf_projection"))
            .map(to_ec_value)
            .transpose()?,
        extra: copy_extra(
            obj,
            &[
                "namespace",
                "metricName",
                "metric_name",
                "timestampMs",
                "timestamp_ms",
                "dimensions",
                "values",
                "largeFleetWorkaround",
                "large_fleet_workaround",
                "emfProjection",
                "emf_projection",
            ],
        )?,
    })
}

fn from_metric_update(metric: pb::MetricUpdate) -> Value {
    let mut obj = Map::new();
    if !metric.namespace.is_empty() {
        obj.insert("namespace".to_string(), Value::String(metric.namespace));
    }
    if !metric.metric_name.is_empty() {
        obj.insert("metricName".to_string(), Value::String(metric.metric_name));
    }
    if metric.timestamp_ms != 0 {
        obj.insert(
            "timestampMs".to_string(),
            Value::Number(metric.timestamp_ms.into()),
        );
    }
    if !metric.dimensions.is_empty() {
        obj.insert(
            "dimensions".to_string(),
            Value::Object(
                metric
                    .dimensions
                    .into_iter()
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect(),
            ),
        );
    }
    if !metric.values.is_empty() {
        obj.insert(
            "values".to_string(),
            Value::Array(
                metric
                    .values
                    .into_iter()
                    .map(|value| {
                        let mut obj = Map::new();
                        if !value.name.is_empty() {
                            obj.insert("name".to_string(), Value::String(value.name));
                        }
                        if let Some(n) = serde_json::Number::from_f64(value.value) {
                            obj.insert("value".to_string(), Value::Number(n));
                        }
                        if !value.unit.is_empty() {
                            obj.insert("unit".to_string(), Value::String(value.unit));
                        }
                        if value.storage_resolution != 0 {
                            obj.insert(
                                "storageResolution".to_string(),
                                Value::Number(value.storage_resolution.into()),
                            );
                        }
                        Value::Object(obj)
                    })
                    .collect(),
            ),
        );
    }
    if metric.large_fleet_workaround {
        obj.insert("largeFleetWorkaround".to_string(), Value::Bool(true));
    }
    if let Some(emf) = metric.emf_projection {
        obj.insert("emfProjection".to_string(), from_ec_value(emf));
    }
    extend_extra(&mut obj, metric.extra);
    Value::Object(obj)
}

fn to_event(body: &Value) -> Result<pb::EventMessage> {
    let obj = as_object(body, "EventMessage")?;
    Ok(pb::EventMessage {
        severity: str_value(obj, "severity").unwrap_or_default(),
        r#type: str_value(obj, "type").unwrap_or_default(),
        message: str_value(obj, "message"),
        timestamp: str_value(obj, "timestamp").unwrap_or_default(),
        timestamp_ms: u64_value(obj, "timestampMs").or_else(|| u64_value(obj, "timestamp_ms")),
        context: obj.get("context").map(to_ec_value).transpose()?,
        alarm: obj.get("alarm").and_then(Value::as_bool),
        active: obj.get("active").and_then(Value::as_bool),
        extra: copy_extra(
            obj,
            &[
                "severity",
                "type",
                "message",
                "timestamp",
                "timestampMs",
                "timestamp_ms",
                "context",
                "alarm",
                "active",
            ],
        )?,
    })
}

fn from_event(event: pb::EventMessage) -> Value {
    let mut obj = Map::new();
    if !event.severity.is_empty() {
        obj.insert("severity".to_string(), Value::String(event.severity));
    }
    if !event.r#type.is_empty() {
        obj.insert("type".to_string(), Value::String(event.r#type));
    }
    if let Some(message) = event.message {
        obj.insert("message".to_string(), Value::String(message));
    }
    if !event.timestamp.is_empty() {
        obj.insert("timestamp".to_string(), Value::String(event.timestamp));
    }
    if let Some(timestamp_ms) = event.timestamp_ms {
        obj.insert(
            "timestampMs".to_string(),
            Value::Number(timestamp_ms.into()),
        );
    }
    if let Some(context) = event.context {
        obj.insert("context".to_string(), from_ec_value(context));
    }
    if let Some(alarm) = event.alarm {
        obj.insert("alarm".to_string(), Value::Bool(alarm));
    }
    if let Some(active) = event.active {
        obj.insert("active".to_string(), Value::Bool(active));
    }
    extend_extra(&mut obj, event.extra);
    Value::Object(obj)
}

fn to_command(header_name: &str, body: &Value) -> Result<pb::CommandMessage> {
    let obj = as_object(body, "CommandMessage")?;
    let wrapped_payload = !obj.contains_key("payload")
        && !obj.contains_key("ok")
        && !obj.contains_key("result")
        && !obj.contains_key("error");
    Ok(pb::CommandMessage {
        verb: str_value(obj, "verb").unwrap_or_else(|| header_name.to_string()),
        payload: if wrapped_payload {
            Some(to_ec_value(body)?)
        } else {
            obj.get("payload").map(to_ec_value).transpose()?
        },
        ok: obj.get("ok").and_then(Value::as_bool),
        result: obj.get("result").map(to_ec_value).transpose()?,
        error: obj
            .get("error")
            .and_then(Value::as_object)
            .map(to_command_error)
            .transpose()?,
        extra: if wrapped_payload {
            Default::default()
        } else {
            copy_extra(obj, &["verb", "payload", "ok", "result", "error"])?
        },
    })
}

fn to_command_error(obj: &Map<String, Value>) -> Result<pb::CommandError> {
    Ok(pb::CommandError {
        code: str_value(obj, "code").unwrap_or_default(),
        message: str_value(obj, "message").unwrap_or_default(),
        details: obj
            .get("details")
            .and_then(Value::as_object)
            .map(|details| {
                details
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), to_ec_value(value)?)))
                    .collect::<Result<_>>()
            })
            .transpose()?
            .unwrap_or_default(),
    })
}

fn from_command(command: pb::CommandMessage) -> Value {
    let pure_payload = command.payload.is_some()
        && command.ok.is_none()
        && command.result.is_none()
        && command.error.is_none()
        && command.extra.is_empty();
    if pure_payload {
        return command.payload.map(from_ec_value).unwrap_or(Value::Null);
    }
    let mut obj = Map::new();
    if !command.verb.is_empty() {
        obj.insert("verb".to_string(), Value::String(command.verb));
    }
    if let Some(payload) = command.payload {
        obj.insert("payload".to_string(), from_ec_value(payload));
    }
    if let Some(ok) = command.ok {
        obj.insert("ok".to_string(), Value::Bool(ok));
    }
    if let Some(result) = command.result {
        obj.insert("result".to_string(), from_ec_value(result));
    }
    if let Some(error) = command.error {
        let mut err = Map::new();
        if !error.code.is_empty() {
            err.insert("code".to_string(), Value::String(error.code));
        }
        if !error.message.is_empty() {
            err.insert("message".to_string(), Value::String(error.message));
        }
        if !error.details.is_empty() {
            err.insert(
                "details".to_string(),
                Value::Object(
                    error
                        .details
                        .into_iter()
                        .map(|(key, value)| (key, from_ec_value(value)))
                        .collect(),
                ),
            );
        }
        obj.insert("error".to_string(), Value::Object(err));
    }
    extend_extra(&mut obj, command.extra);
    Value::Object(obj)
}

fn as_object<'a>(value: &'a Value, name: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| EdgeCommonsError::Messaging(format!("{name} body must be a JSON object")))
}

fn str_value(obj: &Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn u64_value(obj: &Map<String, Value>, key: &str) -> Option<u64> {
    obj.get(key).and_then(Value::as_u64)
}

fn f64_value(obj: &Map<String, Value>, key: &str) -> Option<f64> {
    obj.get(key).and_then(Value::as_f64)
}

fn copy_extra(obj: &Map<String, Value>, known: &[&str]) -> Result<BTreeMap<String, pb::EcValue>> {
    obj.iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| Ok((key.clone(), to_ec_value(value)?)))
        .collect()
}

fn extend_extra(obj: &mut Map<String, Value>, extra: BTreeMap<String, pb::EcValue>) {
    for (key, value) in extra {
        obj.insert(key, from_ec_value(value));
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
/// ([`Self::instance`]; absent/empty ⇒ component scope, D-U28); with neither,
/// `identity` stays `None` (bootstrap/raw messages legally omit it).
#[derive(Debug, Clone)]
pub struct MessageBuilder {
    header: MessageHeader,
    tags: Option<BTreeMap<String, Value>>,
    body: Value,
    content_type: Option<String>,
    content_encoding: Option<String>,
    schema: Option<MessageBodySchema>,
    body_case: MessageBodyCase,
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
        let timestamp = now_rfc3339();
        let timestamp_ms = timestamp_ms_from_rfc3339(&timestamp);
        Self {
            header: MessageHeader {
                name: name.into(),
                version: version.into(),
                timestamp,
                timestamp_ms,
                correlation_id: Uuid::new_v4().to_string(),
                uuid: Uuid::new_v4().to_string(),
                reply_to: None,
            },
            tags: None,
            body: Value::Null,
            content_type: None,
            content_encoding: None,
            schema: None,
            body_case: MessageBodyCase::BodyNotSet,
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

    /// Set a structured body explicitly.
    pub fn structured_payload(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::Structured;
        self
    }

    /// Set a structured body explicitly.
    pub fn structured_body(self, body: Value) -> Self {
        self.structured_payload(body)
    }

    /// Set a `SouthboundSignalUpdate` typed body explicitly.
    pub fn southbound_signal_update(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::SouthboundSignalUpdate;
        self
    }

    /// Set a `StateUpdate` typed body explicitly.
    pub fn state_update(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::StateUpdate;
        self
    }

    /// Set a `ConfigUpdate` typed body explicitly.
    pub fn config_update(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::ConfigUpdate;
        self
    }

    /// Set a `MetricUpdate` typed body explicitly.
    pub fn metric_update(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::MetricUpdate;
        self
    }

    /// Set an `EventMessage` typed body explicitly.
    pub fn event(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::Event;
        self
    }

    /// Set a `CommandMessage` typed body explicitly.
    pub fn command(mut self, body: Value) -> Self {
        self.body = body;
        self.body_case = MessageBodyCase::Command;
        self
    }

    /// Set the message body from bytes using the first-class bounded binary marker.
    ///
    /// # Errors
    /// [`EdgeCommonsError::Messaging`] when the decoded body exceeds
    /// [`MAX_BINARY_BODY_BYTES`].
    pub fn binary_payload(mut self, bytes: impl AsRef<[u8]>) -> Result<Self> {
        self.body = binary_body_value(bytes.as_ref())?;
        self.body_case = MessageBodyCase::Opaque;
        if self.content_type.is_none() {
            self.content_type = Some(DEFAULT_OPAQUE_CONTENT_TYPE.to_string());
        }
        Ok(self)
    }

    /// Set an opaque byte body and content type.
    pub fn opaque_payload(
        mut self,
        bytes: impl AsRef<[u8]>,
        content_type: impl Into<String>,
    ) -> Result<Self> {
        self.body = binary_body_value(bytes.as_ref())?;
        self.body_case = MessageBodyCase::Opaque;
        self.content_type = Some(content_type.into());
        Ok(self)
    }

    /// Set an opaque byte body and content type.
    pub fn opaque_body(
        self,
        bytes: impl AsRef<[u8]>,
        content_type: impl Into<String>,
    ) -> Result<Self> {
        self.opaque_payload(bytes, content_type)
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
        self.header.timestamp_ms = timestamp_ms_from_rfc3339(&self.header.timestamp);
        self
    }

    /// Pin the canonical protobuf timestamp in epoch milliseconds.
    pub fn timestamp_ms(mut self, timestamp_ms: u64) -> Self {
        self.header.timestamp_ms = timestamp_ms;
        self.header.timestamp = rfc3339_from_timestamp_ms(timestamp_ms);
        self
    }

    /// Set the reply-to topic, marking this as a request.
    pub fn reply_to(mut self, topic: impl Into<String>) -> Self {
        self.header.reply_to = Some(topic.into());
        self
    }

    /// Add a single tag (creates the envelope `tags` member if absent).
    pub fn tag(mut self, key: impl Into<String>, value: Value) -> Self {
        self.tags
            .get_or_insert_with(BTreeMap::new)
            .insert(key.into(), value);
        self
    }

    /// Replace all envelope tags.
    pub fn tags(mut self, tags: MessageTags) -> Self {
        self.tags = Some(tags.extra);
        self
    }

    /// Set body content type metadata.
    pub fn content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    /// Set body content encoding metadata.
    pub fn content_encoding(mut self, content_encoding: impl Into<String>) -> Self {
        self.content_encoding = Some(content_encoding.into());
        self
    }

    /// Set body schema metadata.
    pub fn schema(mut self, schema: MessageBodySchema) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Set the body case explicitly.
    pub fn body_case(mut self, body_case: MessageBodyCase) -> Self {
        self.body_case = body_case;
        self
    }

    /// Set the per-message instance token stamped into the identity element. An
    /// absent/empty token means component scope (D-U28: the identity carries no
    /// `instance`). Only takes effect when a config-resolved identity is stamped (an
    /// explicit [`Self::identity`] override is stamped verbatim).
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
                // D-U28: an absent/empty instance token stamps component scope (no instance).
                component_identity.with_instance_infallible(self.instance.as_deref())
            })
        };
        Message {
            header: self.header,
            identity,
            tags: self.tags.map(|extra| MessageTags { extra }),
            content_type: self.content_type,
            content_encoding: self.content_encoding,
            schema: self.schema,
            body_case: self.body_case,
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

fn timestamp_ms_from_rfc3339(timestamp: &str) -> u64 {
    let Ok(parsed) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return 0;
    };
    let millis = parsed.unix_timestamp_nanos() / 1_000_000;
    u64::try_from(millis).unwrap_or(0)
}

fn rfc3339_from_timestamp_ms(timestamp_ms: u64) -> String {
    let secs = (timestamp_ms / 1000).min(i64::MAX as u64) as i64;
    let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
    let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) else {
        return "1970-01-01T00:00:00Z".to_string();
    };
    let Ok(dt) = dt.replace_nanosecond(nanos) else {
        return "1970-01-01T00:00:00Z".to_string();
    };
    dt.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_identity() -> MessageIdentity {
        MessageIdentity::new(
            vec![
                HierEntry {
                    level: "site".into(),
                    value: "dallas".into(),
                },
                HierEntry {
                    level: "device".into(),
                    value: "gw-01".into(),
                },
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
        assert!(m.header.timestamp_ms > 0);
        assert!(m.header.reply_to.is_none());
        assert!(
            m.identity.is_none(),
            "no config, no override => no identity"
        );
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
        assert_eq!(
            m.header.correlation_id,
            "00000000-0000-4000-8000-000000000002"
        );
    }

    #[test]
    fn protobuf_round_trips_with_expected_shape() {
        let m = MessageBuilder::new("ProcessData", "1.0")
            .payload(json!({ "v": 42 }))
            .tag("site", json!("factory-1"))
            .correlation_id("corr-123")
            .reply_to("reply/here")
            .build();

        let bytes = m.to_vec().unwrap();
        assert_ne!(
            bytes.first(),
            Some(&b'{'),
            "wire payload is protobuf, not JSON"
        );
        let value: Value = serde_json::to_value(&m).unwrap();
        assert_eq!(value["header"]["name"], "ProcessData");
        // Wire keys are snake_case, matching Java/Python.
        assert_eq!(value["header"]["correlation_id"], "corr-123");
        assert_eq!(value["header"]["reply_to"], "reply/here");
        assert_eq!(value["tags"]["site"], "factory-1");
        assert!(
            value.get("identity").is_none(),
            "identity omitted when absent"
        );
        assert_eq!(value["body"]["v"], 42);

        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back.header.name, m.header.name);
        assert_eq!(back.header.version, m.header.version);
        assert_eq!(back.header.timestamp_ms, m.header.timestamp_ms);
        assert_eq!(back.header.correlation_id, m.header.correlation_id);
        assert_eq!(back.header.reply_to, m.header.reply_to);
        assert_eq!(back.tags, m.tags);
        assert_eq!(back.body, m.body);
        assert_eq!(back.body_case(), MessageBodyCase::Structured);
    }

    #[test]
    fn binary_payload_serializes_as_marker_and_decodes() {
        let bytes = [0, 1, 2, 254, 255];
        let m = MessageBuilder::new("Bin", "1.0")
            .binary_payload(bytes)
            .unwrap()
            .build();

        let value: Value = serde_json::to_value(&m).unwrap();

        assert_eq!(value["body"]["_edgecommonsBinary"]["encoding"], "base64");
        assert_eq!(value["body"]["_edgecommonsBinary"]["length"], 5);
        assert_eq!(value["body"]["_edgecommonsBinary"]["data"], "AAEC/v8=");
        assert!(m.is_binary_body());
        assert_eq!(m.body_case(), MessageBodyCase::Opaque);
        assert_eq!(m.content_type.as_deref(), Some(DEFAULT_OPAQUE_CONTENT_TYPE));
        assert_eq!(m.binary_body().unwrap().unwrap(), bytes);

        let back = Message::from_slice(&m.to_vec().unwrap()).unwrap();
        assert!(back.is_binary_body());
        assert_eq!(back.binary_body().unwrap().unwrap(), bytes);
    }

    #[test]
    fn protobuf_preserves_identity_tags_and_content_metadata() {
        let m = MessageBuilder::new("FramePreview", "1.0")
            .uuid("00000000-0000-4000-8000-000000000001")
            .timestamp_ms(1_783_360_800_000)
            .correlation_id("corr-1")
            .identity(test_identity())
            .tag("priority", json!(5_u64))
            .content_type("application/json")
            .content_encoding("gzip")
            .schema(MessageBodySchema {
                name: Some("demo.Frame".to_string()),
                version: Some("1".to_string()),
                content_type: Some("application/x-protobuf".to_string()),
                descriptor_ref: Some("s3://bucket/edgecommons-v1.desc".to_string()),
                hash: Some("sha256:abc".to_string()),
            })
            .structured_payload(json!({ "ok": true }))
            .build();

        let back = Message::from_slice(&m.to_vec().unwrap()).unwrap();
        assert_eq!(back.header.timestamp_ms, 1_783_360_800_000);
        assert_eq!(back.identity, m.identity);
        assert_eq!(
            back.tags.as_ref().unwrap().extra.get("priority"),
            Some(&json!(5_u64))
        );
        assert_eq!(back.content_type.as_deref(), Some("application/json"));
        assert_eq!(back.content_encoding.as_deref(), Some("gzip"));
        let schema = back.schema.as_ref().expect("schema round-trips");
        assert_eq!(schema.name.as_deref(), Some("demo.Frame"));
        assert_eq!(
            schema.descriptor_ref.as_deref(),
            Some("s3://bucket/edgecommons-v1.desc")
        );
        assert_eq!(back.body_case(), MessageBodyCase::Structured);
    }

    #[test]
    fn opaque_body_round_trips_with_content_type() {
        let m = MessageBuilder::new("FramePreview", "1.0")
            .opaque_payload([0xff, 0xd8, 0xff, 0xe0], "image/jpeg")
            .unwrap()
            .build();

        let back = Message::from_slice(&m.to_vec().unwrap()).unwrap();
        assert_eq!(back.body_case(), MessageBodyCase::Opaque);
        assert_eq!(back.content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(
            back.opaque_body().unwrap().unwrap(),
            vec![0xff, 0xd8, 0xff, 0xe0]
        );
    }

    #[test]
    fn reserved_names_infer_typed_bodies() {
        let cases = [
            (
                "Telemetry",
                json!({
                    "signal": { "id": "temp" },
                    "samples": [ { "value": 21.5, "quality": "GOOD" } ]
                }),
                MessageBodyCase::SouthboundSignalUpdate,
            ),
            (
                "state",
                json!({
                    "status": "RUNNING",
                    "instances": [ { "instance": "main", "connected": true } ]
                }),
                MessageBodyCase::StateUpdate,
            ),
            (
                "cfg",
                json!({ "config": { "pollMs": 1000 } }),
                MessageBodyCase::ConfigUpdate,
            ),
            (
                "Metric",
                json!({
                    "namespace": "EdgeCommons",
                    "metricName": "Messages",
                    "timestampMs": 1_783_360_800_000_u64,
                    "values": [ { "name": "Count", "value": 2.0, "unit": "Count" } ]
                }),
                MessageBodyCase::MetricUpdate,
            ),
            (
                "evt",
                json!({
                    "severity": "INFO",
                    "type": "Lifecycle",
                    "message": "started",
                    "timestamp": "2026-07-06T18:00:00Z"
                }),
                MessageBodyCase::Event,
            ),
        ];

        for (name, body, expected) in cases {
            let message = MessageBuilder::new(name, "1.0").payload(body).build();
            let bytes = message.to_vec().unwrap();
            let decoded = pb::EdgeCommonsMessage::decode(bytes.as_slice()).unwrap();
            let actual = match decoded.body {
                Some(pb::edge_commons_message::Body::SouthboundSignalUpdate(_)) => {
                    MessageBodyCase::SouthboundSignalUpdate
                }
                Some(pb::edge_commons_message::Body::StateUpdate(_)) => {
                    MessageBodyCase::StateUpdate
                }
                Some(pb::edge_commons_message::Body::ConfigUpdate(_)) => {
                    MessageBodyCase::ConfigUpdate
                }
                Some(pb::edge_commons_message::Body::MetricUpdate(_)) => {
                    MessageBodyCase::MetricUpdate
                }
                Some(pb::edge_commons_message::Body::Event(_)) => MessageBodyCase::Event,
                other => panic!("unexpected protobuf body case for {name}: {other:?}"),
            };
            assert_eq!(actual, expected, "wrong protobuf body case for {name}");

            let back = Message::from_slice(&bytes).unwrap();
            assert_eq!(
                back.body_case(),
                expected,
                "wrong decoded body case for {name}"
            );
        }
    }

    #[test]
    fn explicit_command_body_preserves_component_facing_payload() {
        let m = MessageBuilder::new("setState", "1.0")
            .command(json!({ "status": "RUNNING" }))
            .build();
        let bytes = m.to_vec().unwrap();
        let decoded = pb::EdgeCommonsMessage::decode(bytes.as_slice()).unwrap();
        let command = match decoded.body {
            Some(pb::edge_commons_message::Body::Command(command)) => command,
            other => panic!("expected command body, got {other:?}"),
        };
        assert_eq!(command.verb, "setState");
        assert!(command.extra.is_empty());
        assert_eq!(
            from_ec_value(command.payload.expect("command payload")),
            json!({ "status": "RUNNING" })
        );

        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back.body_case(), MessageBodyCase::Command);
        assert_eq!(back.body, json!({ "status": "RUNNING" }));
    }

    #[test]
    fn canonical_protobuf_vectors_round_trip_exact_bytes() {
        let text = std::fs::read_to_string("../../protobuf-test-vectors/messages.pb.hex").unwrap();
        for line in text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let (id, hex) = line.split_once(' ').unwrap();
            let bytes = decode_hex_for_test(hex);
            let message = Message::from_slice(&bytes).unwrap();
            assert_eq!(encode_hex_for_test(&message.to_vec().unwrap()), hex, "{id}");
        }
    }

    #[test]
    fn southbound_byte_sample_round_trips_as_protobuf_bytes_value() {
        let m = MessageBuilder::new(DATA_MESSAGE_NAME, "1.0")
            .southbound_signal_update(json!({
                "signal": { "id": "camera-1/thumbnail" },
                "samples": [ {
                    "value": {
                        "_edgecommonsBinary": {
                            "encoding": "base64",
                            "length": 5,
                            "data": "AAEC/v8="
                        }
                    },
                    "quality": "GOOD",
                    "sourceTs": "2026-07-06T17:59:59.900Z",
                    "serverTs": "2026-07-06T18:00:00Z"
                } ]
            }))
            .build();

        let bytes = m.to_vec().unwrap();
        let decoded = pb::EdgeCommonsMessage::decode(bytes.as_slice()).unwrap();
        assert!(matches!(
            decoded.body,
            Some(pb::edge_commons_message::Body::SouthboundSignalUpdate(_))
        ));
        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back.body_case(), MessageBodyCase::SouthboundSignalUpdate);
        assert_eq!(
            back.body["samples"][0]["value"]["_edgecommonsBinary"]["data"],
            "AAEC/v8="
        );
        assert_eq!(
            back.body["samples"][0]["sourceTsMs"],
            json!(1_783_360_799_900_u64)
        );
        assert_eq!(
            back.body["samples"][0]["serverTsMs"],
            json!(1_783_360_800_000_u64)
        );
    }

    #[test]
    fn binary_marker_validates_length_and_size() {
        let bytes = serde_json::to_vec(&json!({
            "body": {
                "_edgecommonsBinary": {
                    "encoding": "base64",
                    "length": 4,
                    "data": "AAEC/v8="
                }
            }
        }))
        .unwrap();
        let m: Message = serde_json::from_slice(&bytes).unwrap();
        assert!(m.is_binary_body());
        assert!(m.binary_body().is_err());

        let oversized = vec![0_u8; MAX_BINARY_BODY_BYTES + 1];
        assert!(
            MessageBuilder::new("Bin", "1.0")
                .binary_payload(&oversized)
                .is_err()
        );
    }

    #[test]
    fn identity_serializes_between_header_and_tags_and_round_trips() {
        let m = MessageBuilder::new("state", "1.0")
            .identity(test_identity())
            .payload(json!({ "status": "RUNNING" }))
            .build();
        let bytes = m.to_vec().unwrap();
        let text = serde_json::to_string(&m).unwrap();
        // Canonical member order: header, identity, (tags,) body.
        let header_pos = text.find("\"header\"").unwrap();
        let identity_pos = text.find("\"identity\"").unwrap();
        let body_pos = text.find("\"body\"").unwrap();
        assert!(header_pos < identity_pos && identity_pos < body_pos);

        let value: Value = serde_json::to_value(&m).unwrap();
        assert_eq!(value["identity"]["path"], "dallas/gw-01");
        assert_eq!(value["identity"]["component"], "opcua-adapter");
        // D-U28: test_identity() is component scope (built with None) - the instance key is omitted.
        assert!(value["identity"].get("instance").is_none());
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
        // D-U28: built with None => component scope.
        assert_eq!(id.instance(), None);
        let kep = id.with_instance("kep1").unwrap();
        assert_eq!(kep.instance(), Some("kep1"));
        assert_eq!(kep.device(), "gw-01");
        // D-U28: an empty token is component scope (no longer an error).
        assert_eq!(id.with_instance("").unwrap().instance(), None);
        // D-U28: a reserved class token is rejected.
        assert!(id.with_instance("state").is_err());
    }

    #[test]
    fn identity_constructor_validates() {
        assert!(
            MessageIdentity::new(vec![], "c", None).is_err(),
            "empty hier"
        );
        assert!(
            MessageIdentity::new(
                vec![HierEntry {
                    level: "".into(),
                    value: "v".into()
                }],
                "c",
                None
            )
            .is_err(),
            "empty level"
        );
        assert!(
            MessageIdentity::new(
                vec![HierEntry {
                    level: "device".into(),
                    value: "".into()
                }],
                "c",
                None
            )
            .is_err(),
            "empty value"
        );
        assert!(
            MessageIdentity::new(
                vec![HierEntry {
                    level: "device".into(),
                    value: "d".into()
                }],
                "",
                None
            )
            .is_err(),
            "empty component"
        );
    }

    #[test]
    fn d_u28_reserved_class_token_instance_is_rejected() {
        // A present instance may not equal any of the eight UNS class tokens (D-U28).
        for token in ["state", "metric", "cfg", "log", "data", "evt", "cmd", "app"] {
            assert!(
                MessageIdentity::new(
                    vec![HierEntry {
                        level: "device".into(),
                        value: "gw-01".into()
                    }],
                    "c",
                    Some(token.to_string()),
                )
                .is_err(),
                "instance '{token}' must be rejected"
            );
        }
        // A non-class token is fine.
        assert_eq!(
            MessageIdentity::new(
                vec![HierEntry {
                    level: "device".into(),
                    value: "gw-01".into()
                }],
                "c",
                Some("kep1".to_string()),
            )
            .unwrap()
            .instance(),
            Some("kep1")
        );
    }

    #[test]
    fn d_u28_component_scope_identity_omits_instance_and_round_trips() {
        // A component-scope identity serializes without the `instance` key and survives
        // the protobuf round trip as component scope (empty proto instance ⇒ None).
        let id = MessageIdentity::new(
            vec![HierEntry {
                level: "device".into(),
                value: "gw-01".into(),
            }],
            "opcua-adapter",
            None,
        )
        .unwrap();
        let m = MessageBuilder::new("state", "1.0")
            .identity(id)
            .payload(json!({ "status": "RUNNING" }))
            .build();
        assert!(
            serde_json::to_value(&m).unwrap()["identity"]
                .get("instance")
                .is_none()
        );
        let back = Message::from_slice(&m.to_vec().unwrap()).unwrap();
        assert_eq!(back.identity.unwrap().instance(), None);
    }

    #[test]
    fn d_u28_wire_parse_drops_reserved_instance_token() {
        // A peer sending an instance equal to a class token yields a dropped (None) identity.
        assert!(
            MessageIdentity::from_wire(&json!({
                "hier": [ { "level": "device", "value": "gw-01" } ],
                "component": "c",
                "instance": "state"
            }))
            .is_none()
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
        // D-U28: a missing instance parses to component scope (None).
        assert_eq!(parsed.instance(), None);
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
        assert!(
            MessageIdentity::from_wire(&json!({ "hier": ["bad"], "component": "c" })).is_none()
        );
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
        let m: Message = serde_json::from_slice(&bytes).unwrap();
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
        let m: Message = serde_json::from_slice(&bytes).unwrap();
        assert!(
            !m.is_raw(),
            "an object with an identity member is an envelope"
        );
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
        let m: Message = serde_json::from_slice(&bytes).unwrap();
        let tags = m.tags.unwrap();
        assert_eq!(tags.extra.get("thing"), Some(&json!("legacy-thing")));
        assert_eq!(tags.extra.get("site"), Some(&json!("f1")));
    }

    #[test]
    fn reply_to_is_omitted_when_absent() {
        let m = MessageBuilder::new("N", "1.0").build();
        let value: Value = serde_json::to_value(&m).unwrap();
        assert!(value["header"].get("reply_to").is_none());
    }

    #[test]
    fn diagnostic_json_deserialize_keeps_non_envelope_object_as_raw() {
        // A payload with none of header/identity/tags/body is delivered raw.
        let bytes = serde_json::to_vec(&json!({ "temperature": 21.5, "ok": true })).unwrap();
        let m: Message = serde_json::from_slice(&bytes).unwrap();
        assert!(m.is_raw());
        assert_eq!(m.get_raw().unwrap()["temperature"], 21.5);
    }

    #[test]
    fn normal_wire_rejects_non_protobuf_payload() {
        assert!(Message::from_slice(b"not json at all").is_err());
        assert!(
            Message::from_slice(br#"{"temperature":21.5}"#).is_err(),
            "JSON/raw payloads are not EdgeCommons protobuf messages"
        );
    }

    #[test]
    fn raw_message_serializes_under_raw_key() {
        let m = Message::raw(json!({ "x": 1 }));
        let value: Value = serde_json::to_value(&m).unwrap();
        assert_eq!(value, json!({ "raw": { "x": 1 } }));
        assert!(
            m.to_vec().is_err(),
            "raw messages are not EdgeCommons envelopes"
        );
    }

    #[test]
    fn envelope_with_missing_parts_defaults_them() {
        // Body-only object is still an envelope (it has the `body` key).
        let bytes = serde_json::to_vec(&json!({ "body": { "v": 1 } })).unwrap();
        let m: Message = serde_json::from_slice(&bytes).unwrap();
        assert!(!m.is_raw());
        assert_eq!(m.body, json!({ "v": 1 }));
        assert_eq!(m.header, MessageHeader::default());
        assert!(m.identity.is_none());
        assert!(m.tags.is_none());
    }

    fn decode_hex_for_test(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0);
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    fn encode_hex_for_test(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
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
        // D-U28: a config-stamped message with no explicit instance is component scope.
        assert_eq!(identity.instance(), None);
        let tags = m.tags.expect("config-bound builder stamps tags");
        assert_eq!(tags.extra.get("site"), Some(&json!("f1")));
        assert!(
            !tags.extra.contains_key("thing"),
            "no synthesized thing tag (hard cut)"
        );
    }

    #[test]
    fn instance_token_applies_to_config_identity_only() {
        let cfg = Config::from_value("c", "thing-9", json!({})).unwrap();
        let m = MessageBuilder::new("N", "1.0")
            .from_config(&cfg)
            .instance("kep1")
            .build();
        assert_eq!(m.identity.unwrap().instance(), Some("kep1"));

        // An explicit override is stamped verbatim; the instance token is ignored.
        // (test_identity() is component scope, D-U28.)
        let m = MessageBuilder::new("N", "1.0")
            .from_config(&cfg)
            .identity(test_identity())
            .instance("kep1")
            .build();
        assert_eq!(m.identity.unwrap().instance(), None);

        // D-U28: an empty instance token means component scope (no instance).
        let m = MessageBuilder::new("N", "1.0")
            .from_config(&cfg)
            .instance("")
            .build();
        assert_eq!(m.identity.unwrap().instance(), None);
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

    // ===================== inbound binary-marker validation =====================

    /// An inbound envelope whose body is the given binary-marker descriptor.
    fn message_with_marker(descriptor: Value) -> Message {
        serde_json::from_value(json!({ "body": { BINARY_BODY_KEY: descriptor } }))
            .expect("a syntactically valid envelope")
    }

    #[test]
    fn a_malformed_inbound_binary_marker_is_rejected_rather_than_decoded() {
        // Every one of these is a *hostile or corrupt* peer's payload. `binary_body()` is the
        // only door to the bytes, so each must fail closed — never yield truncated, over-long,
        // or garbage bytes to the component.
        let cases: [(&str, Value); 6] = [
            (
                "encoding must be base64",
                json!({ "encoding": "hex", "length": 1, "data": "AA==" }),
            ),
            (
                "encoding must be base64",
                json!({ "length": 1, "data": "AA==" }), // encoding absent entirely
            ),
            (
                "length must be a non-negative integer",
                json!({ "encoding": "base64", "data": "AA==" }),
            ),
            (
                "data is required",
                json!({ "encoding": "base64", "length": 1 }),
            ),
            (
                "not valid base64",
                json!({ "encoding": "base64", "length": 1, "data": "%%%%" }),
            ),
            (
                "length does not match decoded data",
                json!({ "encoding": "base64", "length": 99, "data": "AA==" }),
            ),
        ];

        for (expected, descriptor) in cases {
            let message = message_with_marker(descriptor.clone());
            assert!(
                message.is_binary_body(),
                "the marker key alone identifies a binary body: {descriptor}"
            );
            let error = message
                .binary_body()
                .expect_err("a malformed marker must not decode");
            assert!(
                error.to_string().contains(expected),
                "expected '{expected}', got '{error}' for {descriptor}"
            );
        }
    }

    #[test]
    fn an_inbound_binary_body_larger_than_the_cap_is_refused_before_allocation() {
        // The declared length is attacker-controlled: it must be bounded *before* it is used,
        // so a 4 GiB claim cannot become a 4 GiB allocation.
        let error = message_with_marker(json!({
            "encoding": "base64",
            "length": (MAX_BINARY_BODY_BYTES as u64) + 1,
            "data": "AA=="
        }))
        .binary_body()
        .expect_err("an over-cap declared length must be refused");
        assert!(error.to_string().contains("exceeds"), "{error}");
    }

    #[test]
    fn a_binary_marker_that_is_not_an_object_is_rejected() {
        let error = message_with_marker(json!("not-a-descriptor"))
            .binary_body()
            .expect_err("the marker's value must be a descriptor object");
        assert!(error.to_string().contains("must be an object"), "{error}");
    }

    #[test]
    fn a_non_binary_body_yields_no_bytes_instead_of_an_error() {
        let plain = MessageBuilder::new("N", "1.0")
            .payload(json!({ "temperature": 21.5 }))
            .build();
        assert!(!plain.is_binary_body());
        assert_eq!(plain.binary_body().unwrap(), None);

        // A body that is not even an object cannot carry a marker.
        let scalar = MessageBuilder::new("N", "1.0").payload(json!(7)).build();
        assert_eq!(scalar.binary_body().unwrap(), None);
    }

    // ===================== command error + schema wire shape =====================

    #[test]
    fn a_coded_command_error_with_details_survives_the_protobuf_round_trip() {
        // The command reply's `error` object is the contract every language's command client
        // reads. It travels in the typed `CommandMessage` lane, not as free-form JSON.
        let body = json!({
            "ok": false,
            "error": {
                "code": "DEVICE_BUSY",
                "message": "the sensor is capturing",
                "details": { "retryAfterSecs": 5, "session": "s-7" }
            }
        });
        let message = MessageBuilder::new("sb/capture", "1.0")
            .command(body.clone())
            .build();

        let bytes = message.to_vec().unwrap();
        let decoded = pb::EdgeCommonsMessage::decode(bytes.as_slice()).unwrap();
        let command = match decoded.body {
            Some(pb::edge_commons_message::Body::Command(command)) => command,
            other => panic!("expected the typed command lane, got {other:?}"),
        };
        let error = command.error.expect("the error travels in the typed lane");
        assert_eq!(error.code, "DEVICE_BUSY");
        assert_eq!(error.message, "the sensor is capturing");
        assert_eq!(
            error.details.len(),
            2,
            "the details map must not be flattened away"
        );

        let back = Message::from_slice(&bytes).unwrap();
        assert_eq!(back.body_case(), MessageBodyCase::Command);
        assert_eq!(
            back.body["error"], body["error"],
            "the coded error, its message and every detail survive verbatim"
        );
        assert_eq!(back.body["ok"], json!(false));
        assert_eq!(
            back.body["verb"], "sb/capture",
            "a reply-shaped command body carries the verb back out of the typed lane"
        );
    }

    #[test]
    fn body_schema_metadata_round_trips_through_the_diagnostic_json_form() {
        // Protobuf is the wire; the JSON form is the diagnostic/interop rendering, and it must
        // carry the same schema metadata (name/version/content_type/descriptor_ref/hash).
        let message = MessageBuilder::new("Frame", "1.0")
            .opaque_payload(b"\x01\x02", "application/x-protobuf")
            .unwrap()
            .schema(MessageBodySchema {
                name: Some("demo.Frame".to_string()),
                version: Some("2".to_string()),
                content_type: Some("application/x-protobuf".to_string()),
                descriptor_ref: Some("s3://schemas/demo".to_string()),
                hash: Some("abc123".to_string()),
            })
            .build();

        let json_form = serde_json::to_value(&message).unwrap();
        assert_eq!(
            json_form["schema"],
            json!({
                "name": "demo.Frame",
                "version": "2",
                "content_type": "application/x-protobuf",
                "descriptor_ref": "s3://schemas/demo",
                "hash": "abc123"
            })
        );

        let back: Message = serde_json::from_value(json_form).unwrap();
        let schema = back
            .schema
            .as_ref()
            .expect("the schema survives the JSON round trip");
        assert_eq!(schema.name.as_deref(), Some("demo.Frame"));
        assert_eq!(schema.version.as_deref(), Some("2"));
        assert_eq!(schema.descriptor_ref.as_deref(), Some("s3://schemas/demo"));
        assert_eq!(schema.hash.as_deref(), Some("abc123"));
        assert_eq!(
            back.binary_body().unwrap(),
            Some(b"\x01\x02".to_vec()),
            "the opaque bytes travel alongside their schema"
        );
    }

    #[test]
    fn a_partial_body_schema_omits_its_absent_members() {
        let message = MessageBuilder::new("Frame", "1.0")
            .payload(json!({}))
            .schema(MessageBodySchema {
                name: Some("demo.Frame".to_string()),
                ..MessageBodySchema::default()
            })
            .build();
        assert_eq!(
            serde_json::to_value(&message).unwrap()["schema"],
            json!({ "name": "demo.Frame" }),
            "absent schema members must not serialize as nulls"
        );
    }

    #[test]
    fn the_body_case_spellings_match_the_canonical_java_diagnostics() {
        // These strings are the cross-language diagnostic vocabulary (Java `getBodyCase()`);
        // renaming one silently breaks parity in logs and interop tooling.
        for (case, spelling) in [
            (
                MessageBodyCase::SouthboundSignalUpdate,
                "SOUTHBOUND_SIGNAL_UPDATE",
            ),
            (MessageBodyCase::StateUpdate, "STATE_UPDATE"),
            (MessageBodyCase::ConfigUpdate, "CONFIG_UPDATE"),
            (MessageBodyCase::MetricUpdate, "METRIC_UPDATE"),
            (MessageBodyCase::Event, "EVENT"),
            (MessageBodyCase::Command, "COMMAND"),
            (MessageBodyCase::Structured, "STRUCTURED"),
            (MessageBodyCase::Opaque, "OPAQUE"),
            (MessageBodyCase::BodyNotSet, "BODY_NOT_SET"),
        ] {
            assert_eq!(case.as_str(), spelling);
        }
    }
}
