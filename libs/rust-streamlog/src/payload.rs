use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use prost::Message as ProstMessage;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::config::SinkPayloadFormat;
use crate::error::{EdgeStreamError, Result};
use crate::proto::edgecommons::v1 as pb;

const WIRE_KIND: &str = "edgecommons-protobuf-v1";
const DEFAULT_OPAQUE_CONTENT_TYPE: &str = "application/octet-stream";

#[derive(Debug, Clone)]
pub(crate) struct ProjectedPayload {
    pub payload: Vec<u8>,
    #[allow(dead_code)]
    pub metadata: PayloadMetadata,
}

#[derive(Debug, Clone)]
pub(crate) struct PayloadMetadata {
    pub wire_kind: &'static str,
    pub sink_payload_format: &'static str,
    pub original_wire_kind: Option<&'static str>,
    pub header_name: Option<String>,
    pub header_version: Option<String>,
    pub body_case: Option<&'static str>,
    pub identity_path: Option<String>,
    pub identity_component: Option<String>,
    pub identity_instance: Option<String>,
    pub tags_present: bool,
    pub content_type: Option<String>,
}

impl PayloadMetadata {
    fn from_message(
        payload_format: SinkPayloadFormat,
        msg: &pb::EdgeCommonsMessage,
        body_case: Option<&'static str>,
    ) -> Self {
        let content_type = if msg.content_type.is_empty() && body_case == Some("opaque") {
            Some(DEFAULT_OPAQUE_CONTENT_TYPE.to_string())
        } else if msg.content_type.is_empty() {
            None
        } else {
            Some(msg.content_type.clone())
        };
        Self {
            wire_kind: WIRE_KIND,
            sink_payload_format: payload_format.as_str(),
            original_wire_kind: (payload_format == SinkPayloadFormat::Json).then_some(WIRE_KIND),
            header_name: msg
                .header
                .as_ref()
                .map(|h| h.name.clone())
                .filter(|s| !s.is_empty()),
            header_version: msg
                .header
                .as_ref()
                .map(|h| h.version.clone())
                .filter(|s| !s.is_empty()),
            body_case,
            identity_path: msg
                .identity
                .as_ref()
                .map(|i| i.path.clone())
                .filter(|s| !s.is_empty()),
            identity_component: msg
                .identity
                .as_ref()
                .map(|i| i.component.clone())
                .filter(|s| !s.is_empty()),
            identity_instance: msg
                .identity
                .as_ref()
                .map(|i| i.instance.clone())
                .filter(|s| !s.is_empty()),
            tags_present: !msg.tags.is_empty(),
            content_type,
        }
    }

    pub(crate) fn entries(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![
            ("edgecommons.wireKind", self.wire_kind.to_string()),
            (
                "edgecommons.sink.payloadFormat",
                self.sink_payload_format.to_string(),
            ),
        ];
        if let Some(value) = self.original_wire_kind {
            out.push(("edgecommons.originalWireKind", value.to_string()));
        }
        if let Some(value) = &self.header_name {
            out.push(("edgecommons.header.name", value.clone()));
        }
        if let Some(value) = &self.header_version {
            out.push(("edgecommons.header.version", value.clone()));
        }
        if let Some(value) = self.body_case {
            out.push(("edgecommons.bodyCase", value.to_string()));
        }
        if let Some(value) = &self.identity_path {
            out.push(("edgecommons.identity.path", value.clone()));
        }
        if let Some(value) = &self.identity_component {
            out.push(("edgecommons.identity.component", value.clone()));
        }
        if let Some(value) = &self.identity_instance {
            out.push(("edgecommons.identity.instance", value.clone()));
        }
        out.push(("edgecommons.tags.present", self.tags_present.to_string()));
        if let Some(value) = &self.content_type {
            out.push(("content-type", value.clone()));
        }
        out
    }

    fn to_json(&self) -> Value {
        Value::Object(
            self.entries()
                .into_iter()
                .map(|(key, value)| (key.to_string(), Value::String(value)))
                .collect(),
        )
    }
}

pub(crate) fn project_payload(
    payload_format: SinkPayloadFormat,
    payload: &[u8],
) -> Result<ProjectedPayload> {
    let msg = pb::EdgeCommonsMessage::decode(payload)
        .map_err(|e| EdgeStreamError::Sink(format!("protobuf payload projection: {e}")))?;
    let body_case = body_case_name(msg.body.as_ref());
    let metadata = PayloadMetadata::from_message(payload_format, &msg, body_case);
    let payload = match payload_format {
        SinkPayloadFormat::Protobuf => payload.to_vec(),
        SinkPayloadFormat::Json => serde_json::to_vec(&message_to_json(msg, &metadata, body_case)?)
            .map_err(|e| EdgeStreamError::Sink(format!("protobuf payload projection: {e}")))?,
    };
    Ok(ProjectedPayload { payload, metadata })
}

fn message_to_json(
    msg: pb::EdgeCommonsMessage,
    metadata: &PayloadMetadata,
    body_case: Option<&'static str>,
) -> Result<Value> {
    let header = msg.header.ok_or_else(|| {
        EdgeStreamError::Sink("protobuf payload projection: message header is required".into())
    })?;
    if header.name.is_empty() || header.version.is_empty() {
        return Err(EdgeStreamError::Sink(
            "protobuf payload projection: header name and version are required".into(),
        ));
    }

    let mut root = Map::new();
    root.insert("header".to_string(), header_to_json(header));
    if let Some(identity) = msg.identity {
        root.insert("identity".to_string(), identity_to_json(identity));
    }
    if !msg.tags.is_empty() {
        root.insert("tags".to_string(), ec_map_to_json(msg.tags));
    }
    if let Some(content_type) = &metadata.content_type {
        root.insert(
            "content_type".to_string(),
            Value::String(content_type.clone()),
        );
    }
    if !msg.content_encoding.is_empty() {
        root.insert(
            "content_encoding".to_string(),
            Value::String(msg.content_encoding),
        );
    }
    if let Some(schema) = schema_to_json(msg.schema) {
        root.insert("schema".to_string(), schema);
    }
    if let Some(body_case) = body_case {
        root.insert(
            "body_case".to_string(),
            Value::String(body_case.to_string()),
        );
    }
    root.insert("body".to_string(), body_to_json(msg.body)?);
    root.insert("sink_metadata".to_string(), metadata.to_json());
    Ok(Value::Object(root))
}

fn header_to_json(header: pb::Header) -> Value {
    let mut obj = Map::new();
    obj.insert("name".to_string(), Value::String(header.name));
    obj.insert("version".to_string(), Value::String(header.version));
    obj.insert(
        "timestamp".to_string(),
        Value::String(rfc3339_from_timestamp_ms(header.timestamp_ms)),
    );
    obj.insert(
        "timestamp_ms".to_string(),
        Value::Number(header.timestamp_ms.into()),
    );
    obj.insert("uuid".to_string(), Value::String(header.uuid));
    if let Some(correlation_id) = header.correlation_id {
        obj.insert("correlation_id".to_string(), Value::String(correlation_id));
    }
    if let Some(reply_to) = header.reply_to {
        obj.insert("reply_to".to_string(), Value::String(reply_to));
    }
    Value::Object(obj)
}

fn rfc3339_from_timestamp_ms(timestamp_ms: u64) -> String {
    let secs = (timestamp_ms / 1000).min(i64::MAX as u64) as i64;
    let nanos = ((timestamp_ms % 1000) * 1_000_000) as u32;
    match OffsetDateTime::from_unix_timestamp(secs).and_then(|dt| dt.replace_nanosecond(nanos)) {
        Ok(dt) => dt
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        Err(_) => "1970-01-01T00:00:00Z".to_string(),
    }
}

fn identity_to_json(identity: pb::Identity) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "hier".to_string(),
        Value::Array(
            identity
                .hier
                .into_iter()
                .map(|entry| {
                    let mut obj = Map::new();
                    obj.insert("level".to_string(), Value::String(entry.level));
                    obj.insert("value".to_string(), Value::String(entry.value));
                    Value::Object(obj)
                })
                .collect(),
        ),
    );
    obj.insert("path".to_string(), Value::String(identity.path));
    obj.insert("component".to_string(), Value::String(identity.component));
    obj.insert("instance".to_string(), Value::String(identity.instance));
    Value::Object(obj)
}

fn schema_to_json(schema: Option<pb::BodySchema>) -> Option<Value> {
    let schema = schema?;
    let mut obj = Map::new();
    insert_non_empty(&mut obj, "name", schema.name);
    insert_non_empty(&mut obj, "version", schema.version);
    insert_non_empty(&mut obj, "content_type", schema.content_type);
    insert_non_empty(&mut obj, "descriptor_ref", schema.descriptor_ref);
    insert_non_empty(&mut obj, "hash", schema.hash);
    (!obj.is_empty()).then_some(Value::Object(obj))
}

fn body_case_name(body: Option<&pb::edge_commons_message::Body>) -> Option<&'static str> {
    match body {
        Some(pb::edge_commons_message::Body::SouthboundSignalUpdate(_)) => {
            Some("southbound_signal_update")
        }
        Some(pb::edge_commons_message::Body::StateUpdate(_)) => Some("state_update"),
        Some(pb::edge_commons_message::Body::ConfigUpdate(_)) => Some("config_update"),
        Some(pb::edge_commons_message::Body::MetricUpdate(_)) => Some("metric_update"),
        Some(pb::edge_commons_message::Body::Event(_)) => Some("event"),
        Some(pb::edge_commons_message::Body::Command(_)) => Some("command"),
        Some(pb::edge_commons_message::Body::Structured(_)) => Some("structured"),
        Some(pb::edge_commons_message::Body::Opaque(_)) => Some("opaque"),
        None => None,
    }
}

fn body_to_json(body: Option<pb::edge_commons_message::Body>) -> Result<Value> {
    Ok(match body {
        Some(pb::edge_commons_message::Body::SouthboundSignalUpdate(body)) => {
            telemetry_to_json(body)
        }
        Some(pb::edge_commons_message::Body::StateUpdate(body)) => state_to_json(body),
        Some(pb::edge_commons_message::Body::ConfigUpdate(body)) => config_to_json(body),
        Some(pb::edge_commons_message::Body::MetricUpdate(body)) => metric_to_json(body),
        Some(pb::edge_commons_message::Body::Event(body)) => event_to_json(body),
        Some(pb::edge_commons_message::Body::Command(body)) => command_to_json(body),
        Some(pb::edge_commons_message::Body::Structured(value)) => ec_value_to_json(value),
        Some(pb::edge_commons_message::Body::Opaque(bytes)) => {
            Value::String(BASE64_STANDARD.encode(bytes))
        }
        None => Value::Null,
    })
}

fn telemetry_to_json(telemetry: pb::SouthboundSignalUpdate) -> Value {
    let mut obj = Map::new();
    if let Some(signal) = telemetry.signal {
        obj.insert("signal".to_string(), signal_to_json(signal));
    }
    obj.insert(
        "samples".to_string(),
        Value::Array(telemetry.samples.into_iter().map(sample_to_json).collect()),
    );
    extend_extra(&mut obj, telemetry.extra);
    Value::Object(obj)
}

fn signal_to_json(signal: pb::Signal) -> Value {
    let mut obj = Map::new();
    insert_non_empty(&mut obj, "id", signal.id);
    insert_non_empty(&mut obj, "name", signal.name);
    if let Some(address) = signal.address {
        obj.insert("address".to_string(), ec_value_to_json(address));
    }
    extend_extra(&mut obj, signal.extra);
    Value::Object(obj)
}

fn sample_to_json(sample: pb::Sample) -> Value {
    let mut obj = Map::new();
    if let Some(value) = sample.value {
        obj.insert("value".to_string(), ec_value_to_json(value));
    }
    insert_non_empty(&mut obj, "quality", sample.quality);
    if let Some(quality_raw) = sample.quality_raw {
        obj.insert("quality_raw".to_string(), ec_value_to_json(quality_raw));
    }
    if let Some(source_ts) = sample.source_ts {
        obj.insert("source_ts".to_string(), Value::String(source_ts));
    }
    if let Some(source_ts_ms) = sample.source_ts_ms {
        obj.insert(
            "source_ts_ms".to_string(),
            Value::Number(source_ts_ms.into()),
        );
    }
    if let Some(server_ts) = sample.server_ts {
        obj.insert("server_ts".to_string(), Value::String(server_ts));
    }
    if let Some(server_ts_ms) = sample.server_ts_ms {
        obj.insert(
            "server_ts_ms".to_string(),
            Value::Number(server_ts_ms.into()),
        );
    }
    extend_extra(&mut obj, sample.extra);
    Value::Object(obj)
}

fn state_to_json(state: pb::StateUpdate) -> Value {
    let mut obj = Map::new();
    insert_non_empty(&mut obj, "status", state.status);
    if let Some(uptime_secs) = state.uptime_secs {
        obj.insert("uptime_secs".to_string(), Value::Number(uptime_secs.into()));
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
                        insert_non_empty(&mut obj, "instance", item.instance);
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

fn config_to_json(config: pb::ConfigUpdate) -> Value {
    let mut obj = Map::new();
    if let Some(config) = config.config {
        obj.insert("config".to_string(), ec_value_to_json(config));
    }
    extend_extra(&mut obj, config.extra);
    Value::Object(obj)
}

fn metric_to_json(metric: pb::MetricUpdate) -> Value {
    let mut obj = Map::new();
    insert_non_empty(&mut obj, "namespace", metric.namespace);
    insert_non_empty(&mut obj, "metric_name", metric.metric_name);
    if metric.timestamp_ms != 0 {
        obj.insert(
            "timestamp_ms".to_string(),
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
                    .map(|(key, value)| (key, Value::String(value)))
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
                        insert_non_empty(&mut obj, "name", value.name);
                        if let Some(number) = serde_json::Number::from_f64(value.value) {
                            obj.insert("value".to_string(), Value::Number(number));
                        }
                        insert_non_empty(&mut obj, "unit", value.unit);
                        if value.storage_resolution != 0 {
                            obj.insert(
                                "storage_resolution".to_string(),
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
        obj.insert("large_fleet_workaround".to_string(), Value::Bool(true));
    }
    if let Some(emf_projection) = metric.emf_projection {
        obj.insert(
            "emf_projection".to_string(),
            ec_value_to_json(emf_projection),
        );
    }
    extend_extra(&mut obj, metric.extra);
    Value::Object(obj)
}

fn event_to_json(event: pb::EventMessage) -> Value {
    let mut obj = Map::new();
    insert_non_empty(&mut obj, "severity", event.severity);
    insert_non_empty(&mut obj, "type", event.r#type);
    if let Some(message) = event.message {
        obj.insert("message".to_string(), Value::String(message));
    }
    insert_non_empty(&mut obj, "timestamp", event.timestamp);
    if let Some(timestamp_ms) = event.timestamp_ms {
        obj.insert(
            "timestamp_ms".to_string(),
            Value::Number(timestamp_ms.into()),
        );
    }
    if let Some(context) = event.context {
        obj.insert("context".to_string(), ec_value_to_json(context));
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

fn command_to_json(command: pb::CommandMessage) -> Value {
    let mut obj = Map::new();
    insert_non_empty(&mut obj, "verb", command.verb);
    if let Some(payload) = command.payload {
        obj.insert("payload".to_string(), ec_value_to_json(payload));
    }
    if let Some(ok) = command.ok {
        obj.insert("ok".to_string(), Value::Bool(ok));
    }
    if let Some(result) = command.result {
        obj.insert("result".to_string(), ec_value_to_json(result));
    }
    if let Some(error) = command.error {
        let mut err = Map::new();
        insert_non_empty(&mut err, "code", error.code);
        insert_non_empty(&mut err, "message", error.message);
        if !error.details.is_empty() {
            err.insert("details".to_string(), ec_map_to_json(error.details));
        }
        obj.insert("error".to_string(), Value::Object(err));
    }
    extend_extra(&mut obj, command.extra);
    Value::Object(obj)
}

fn ec_value_to_json(value: pb::EcValue) -> Value {
    match value.kind {
        Some(pb::ec_value::Kind::NullValue(_)) | None => Value::Null,
        Some(pb::ec_value::Kind::BoolValue(value)) => Value::Bool(value),
        Some(pb::ec_value::Kind::IntValue(value)) => Value::Number(value.into()),
        Some(pb::ec_value::Kind::UintValue(value)) => Value::Number(value.into()),
        Some(pb::ec_value::Kind::DoubleValue(value)) => serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(pb::ec_value::Kind::StringValue(value)) => Value::String(value),
        Some(pb::ec_value::Kind::BytesValue(value)) => Value::String(BASE64_STANDARD.encode(value)),
        Some(pb::ec_value::Kind::ListValue(list)) => {
            Value::Array(list.values.into_iter().map(ec_value_to_json).collect())
        }
        Some(pb::ec_value::Kind::MapValue(map)) => ec_map_to_json(map.fields),
    }
}

fn ec_map_to_json(values: BTreeMap<String, pb::EcValue>) -> Value {
    Value::Object(
        values
            .into_iter()
            .map(|(key, value)| (key, ec_value_to_json(value)))
            .collect(),
    )
}

fn extend_extra(obj: &mut Map<String, Value>, extra: BTreeMap<String, pb::EcValue>) {
    for (key, value) in extra {
        obj.insert(key, ec_value_to_json(value));
    }
}

fn insert_non_empty(obj: &mut Map<String, Value>, key: &str, value: String) {
    if !value.is_empty() {
        obj.insert(key.to_string(), Value::String(value));
    }
}

#[cfg(test)]
mod tests {
    use prost::Message as ProstMessage;

    use super::*;

    fn header() -> pb::Header {
        pb::Header {
            name: "SouthboundSignalUpdate".to_string(),
            version: "1.0".to_string(),
            timestamp_ms: 1_704_067_200_123,
            uuid: "uuid-1".to_string(),
            correlation_id: Some("corr-1".to_string()),
            reply_to: Some("reply/topic".to_string()),
        }
    }

    #[test]
    fn json_projection_uses_protobuf_field_names_and_base64_bytes() {
        let msg = pb::EdgeCommonsMessage {
            header: Some(header()),
            identity: Some(pb::Identity {
                hier: vec![
                    pb::HierEntry {
                        level: "site".into(),
                        value: "plant-a".into(),
                    },
                    pb::HierEntry {
                        level: "device".into(),
                        value: "gw-01".into(),
                    },
                ],
                path: "plant-a/gw-01".into(),
                component: "opcua".into(),
                instance: "main".into(),
            }),
            tags: [(
                "workOrder".to_string(),
                pb::EcValue {
                    kind: Some(pb::ec_value::Kind::StringValue("wo-9".into())),
                },
            )]
            .into_iter()
            .collect(),
            body: Some(pb::edge_commons_message::Body::SouthboundSignalUpdate(
                pb::SouthboundSignalUpdate {
                    signal: Some(pb::Signal {
                        id: "temp".into(),
                        name: "Temperature".into(),
                        address: Some(pb::EcValue {
                            kind: Some(pb::ec_value::Kind::BytesValue(vec![0, 1, 2])),
                        }),
                        extra: Default::default(),
                    }),
                    samples: vec![pb::Sample {
                        value: Some(pb::EcValue {
                            kind: Some(pb::ec_value::Kind::DoubleValue(21.5)),
                        }),
                        quality: "GOOD".into(),
                        source_ts: Some("2026-07-06T10:00:00Z".into()),
                        source_ts_ms: Some(1_783_330_800_000),
                        server_ts: Some("2026-07-06T10:00:01Z".into()),
                        server_ts_ms: Some(1_783_330_801_000),
                        quality_raw: None,
                        extra: Default::default(),
                    }],
                    extra: Default::default(),
                },
            )),
            ..Default::default()
        };
        let projected = project_payload(SinkPayloadFormat::Json, &msg.encode_to_vec()).unwrap();
        let json: Value = serde_json::from_slice(&projected.payload).unwrap();

        assert_eq!(json["header"]["timestamp_ms"], 1_704_067_200_123u64);
        assert_eq!(json["body_case"], "southbound_signal_update");
        assert_eq!(json["identity"]["path"], "plant-a/gw-01");
        assert_eq!(json["tags"]["workOrder"], "wo-9");
        assert_eq!(json["body"]["signal"]["address"], "AAEC");
        assert_eq!(
            json["body"]["samples"][0]["source_ts_ms"],
            1_783_330_800_000u64
        );
        assert_eq!(
            json["sink_metadata"]["edgecommons.sink.payloadFormat"],
            "json"
        );
        assert_eq!(
            json["sink_metadata"]["edgecommons.originalWireKind"],
            WIRE_KIND
        );
    }

    #[test]
    fn opaque_projection_infers_content_type_and_preserves_protobuf_mode_bytes() {
        let msg = pb::EdgeCommonsMessage {
            header: Some(header()),
            body: Some(pb::edge_commons_message::Body::Opaque(vec![0, 1, 2, 255])),
            ..Default::default()
        };
        let bytes = msg.encode_to_vec();

        let json_projected = project_payload(SinkPayloadFormat::Json, &bytes).unwrap();
        let json: Value = serde_json::from_slice(&json_projected.payload).unwrap();
        assert_eq!(json["body_case"], "opaque");
        assert_eq!(json["content_type"], DEFAULT_OPAQUE_CONTENT_TYPE);
        assert_eq!(json["body"], "AAEC/w==");

        let protobuf_projected = project_payload(SinkPayloadFormat::Protobuf, &bytes).unwrap();
        assert_eq!(protobuf_projected.payload, bytes);
        assert_eq!(protobuf_projected.metadata.sink_payload_format, "protobuf");
        assert!(protobuf_projected.metadata.original_wire_kind.is_none());
    }

    #[test]
    fn malformed_payload_is_a_sink_failure() {
        let err = project_payload(SinkPayloadFormat::Json, b"not protobuf").unwrap_err();
        assert!(err.to_string().contains("protobuf payload projection"));
    }
}
