"""Protobuf codec for the EdgeCommons message envelope.

The canonical schema is the descriptor set generated from ``proto/edgecommons/v1``.
This module uses dynamic protobuf messages so Python consumes that descriptor directly
without owning generated schema sources.
"""

import base64
import hashlib
import math
from dataclasses import dataclass
from datetime import datetime, timezone
from enum import Enum
from functools import lru_cache
from importlib import resources
from pathlib import Path
from typing import Any, Dict, Iterable, Optional

from google.protobuf import descriptor_pb2, descriptor_pool, message_factory
from google.protobuf.message import DecodeError

DEFAULT_OPAQUE_CONTENT_TYPE = "application/octet-stream"
DATA_MESSAGE_NAME = "SouthboundSignalUpdate"
TELEMETRY_MESSAGE_NAME = "Telemetry"


class MessageBodyCase(Enum):
    SOUTHBOUND_SIGNAL_UPDATE = "SOUTHBOUND_SIGNAL_UPDATE"
    STATE_UPDATE = "STATE_UPDATE"
    CONFIG_UPDATE = "CONFIG_UPDATE"
    METRIC_UPDATE = "METRIC_UPDATE"
    EVENT = "EVENT"
    COMMAND = "COMMAND"
    STRUCTURED = "STRUCTURED"
    OPAQUE = "OPAQUE"
    BODY_NOT_SET = "BODY_NOT_SET"


@dataclass(frozen=True)
class MessageBodySchema:
    name: Optional[str] = None
    version: Optional[str] = None
    content_type: Optional[str] = None
    descriptor_ref: Optional[str] = None
    hash: Optional[str] = None

    def to_dict(self) -> dict:
        obj = {}
        if self.name is not None:
            obj["name"] = self.name
        if self.version is not None:
            obj["version"] = self.version
        if self.content_type is not None:
            obj["content_type"] = self.content_type
        if self.descriptor_ref is not None:
            obj["descriptor_ref"] = self.descriptor_ref
        if self.hash is not None:
            obj["hash"] = self.hash
        return obj

    @staticmethod
    def from_dict(src: Optional[dict]) -> Optional["MessageBodySchema"]:
        if src is None:
            return None
        return MessageBodySchema(
            name=_string_or_none(src, "name"),
            version=_string_or_none(src, "version"),
            content_type=_string_or_none(src, "content_type"),
            descriptor_ref=_string_or_none(src, "descriptor_ref"),
            hash=_string_or_none(src, "hash"),
        )


def to_bytes(message) -> bytes:
    return _to_proto(message).SerializeToString(deterministic=True)


def from_bytes(payload: bytes):
    try:
        proto = _new("EdgeCommonsMessage")
        proto.ParseFromString(bytes(payload))
    except (DecodeError, TypeError, ValueError) as exc:
        raise ValueError("Malformed EdgeCommons protobuf message") from exc
    return _from_proto(proto)


def body_case(message) -> MessageBodyCase:
    if message.body_case is not None:
        return _coerce_body_case(message.body_case)
    if message.body is None:
        return MessageBodyCase.BODY_NOT_SET
    if message.is_binary_body():
        return MessageBodyCase.OPAQUE
    header = message.get_header()
    if (
        header is not None
        and header.name in (DATA_MESSAGE_NAME, TELEMETRY_MESSAGE_NAME)
        and isinstance(message.body, dict)
    ):
        return MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE
    if header is not None and isinstance(message.body, dict):
        if header.name.lower() == "state":
            return MessageBodyCase.STATE_UPDATE
        if header.name.lower() == "cfg" or header.name in ("Config", "Configuration"):
            return MessageBodyCase.CONFIG_UPDATE
        if header.name in ("Metric", "metric"):
            return MessageBodyCase.METRIC_UPDATE
        if header.name.lower() == "evt" or header.name == "Event":
            return MessageBodyCase.EVENT
    return MessageBodyCase.STRUCTURED


def to_diagnostic_json(message) -> dict:
    diagnostic = from_bytes(to_bytes(message)).to_dict()
    if message.header is not None and "header" in diagnostic:
        diagnostic["header"]["timestamp_ms"] = message.header.timestamp_ms
    case = body_case(message)
    diagnostic["body_case"] = case.value
    if case is MessageBodyCase.OPAQUE:
        data = message.get_binary_body() or b""
        diagnostic["body"] = {
            "content_type": _content_type_or_default(message.content_type),
            "length": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    return diagnostic


def _to_proto(message):
    if message.header is None:
        raise ValueError("EdgeCommons protobuf message requires a header")
    if not message.header.name or not message.header.version:
        raise ValueError("EdgeCommons protobuf message requires header name and version")

    proto = _new("EdgeCommonsMessage")
    proto.header.CopyFrom(_to_proto_header(message.header))
    if message.identity is not None:
        proto.identity.CopyFrom(_to_proto_identity(message.identity))
    if message.tags is not None:
        for key, value in message.tags.to_dict().items():
            proto.tags[key].CopyFrom(_to_ec_value(value))
    if message.content_type is not None:
        proto.content_type = message.content_type
    if message.content_encoding is not None:
        proto.content_encoding = message.content_encoding
    if message.schema is not None:
        proto.schema.CopyFrom(_to_proto_schema(message.schema))

    case = body_case(message)
    if case is MessageBodyCase.OPAQUE:
        data = message.get_binary_body() or b""
        proto.content_type = _content_type_or_default(message.content_type)
        proto.opaque = data
    elif case is MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE:
        proto.southbound_signal_update.CopyFrom(_to_telemetry(message.body))
    elif case is MessageBodyCase.STATE_UPDATE:
        proto.state_update.CopyFrom(_to_state_update(message.body))
    elif case is MessageBodyCase.CONFIG_UPDATE:
        proto.config_update.CopyFrom(_to_config_update(message.body))
    elif case is MessageBodyCase.METRIC_UPDATE:
        proto.metric_update.CopyFrom(_to_metric_update(message.body))
    elif case is MessageBodyCase.EVENT:
        proto.event.CopyFrom(_to_event_message(message.body))
    elif case is MessageBodyCase.COMMAND:
        proto.command.CopyFrom(_to_command_message(message.header.name, message.body))
    elif case is MessageBodyCase.STRUCTURED:
        proto.structured.CopyFrom(_to_ec_value(message.body))
    return proto


def _from_proto(proto):
    if not proto.HasField("header") or not proto.header.name or not proto.header.version:
        raise ValueError("EdgeCommons protobuf message requires header name and version")

    from edgecommons.messaging.message import Message, MessageHeader, MessageTags
    from edgecommons.messaging.identity import HierEntry, MessageIdentity

    header = MessageHeader(
        proto.header.name,
        proto.header.version,
        proto.header.correlation_id if proto.header.HasField("correlation_id") else None,
        _iso_from_epoch_ms(proto.header.timestamp_ms),
        proto.header.uuid,
        proto.header.reply_to if proto.header.HasField("reply_to") else None,
        timestamp_ms=proto.header.timestamp_ms,
    )
    identity = None
    if proto.HasField("identity"):
        hier = [
            HierEntry(entry.level, entry.value)
            for entry in proto.identity.hier
            if entry.level and entry.value
        ]
        identity = MessageIdentity.from_dict(
            {
                "hier": [{"level": entry.level, "value": entry.value} for entry in hier],
                "path": proto.identity.path,
                "component": proto.identity.component,
                "instance": proto.identity.instance,
            }
        )
        if identity is None:
            raise ValueError("Malformed protobuf identity")

    tags = None
    if proto.tags:
        tags = MessageTags.from_dict({key: _from_ec_value(value) for key, value in proto.tags.items()})

    message = Message(header=header, identity=identity, tags=tags)
    if proto.content_type:
        message.content_type = proto.content_type
    if proto.content_encoding:
        message.content_encoding = proto.content_encoding
    if proto.HasField("schema"):
        message.schema = _from_proto_schema(proto.schema)

    selected = proto.WhichOneof("body")
    if selected == "southbound_signal_update":
        message.body = _from_telemetry(proto.southbound_signal_update)
        message.body_case = MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE
    elif selected == "state_update":
        message.body = _from_state_update(proto.state_update)
        message.body_case = MessageBodyCase.STATE_UPDATE
    elif selected == "config_update":
        message.body = _from_config_update(proto.config_update)
        message.body_case = MessageBodyCase.CONFIG_UPDATE
    elif selected == "metric_update":
        message.body = _from_metric_update(proto.metric_update)
        message.body_case = MessageBodyCase.METRIC_UPDATE
    elif selected == "event":
        message.body = _from_event_message(proto.event)
        message.body_case = MessageBodyCase.EVENT
    elif selected == "command":
        message.body = _from_command_message(proto.command)
        message.body_case = MessageBodyCase.COMMAND
    elif selected == "structured":
        message.body = _from_ec_value(proto.structured)
        message.body_case = MessageBodyCase.STRUCTURED
    elif selected == "opaque":
        message.body = bytes(proto.opaque)
        message.content_type = _content_type_or_default(proto.content_type)
        message.body_case = MessageBodyCase.OPAQUE
    else:
        message.body_case = MessageBodyCase.BODY_NOT_SET
    return message


def _to_proto_header(header):
    proto = _new("Header")
    proto.name = header.name
    proto.version = header.version
    proto.timestamp_ms = int(header.timestamp_ms or 0)
    proto.uuid = header.uuid or ""
    if header.correlation_id is not None:
        proto.correlation_id = header.correlation_id
    if header.reply_to is not None:
        proto.reply_to = header.reply_to
    return proto


def _to_proto_identity(identity):
    proto = _new("Identity")
    proto.path = identity.path
    proto.component = identity.component
    proto.instance = identity.instance
    for entry in identity.hier:
        item = proto.hier.add()
        item.level = entry.level
        item.value = entry.value
    return proto


def _to_proto_schema(schema: MessageBodySchema):
    proto = _new("BodySchema")
    if schema.name is not None:
        proto.name = schema.name
    if schema.version is not None:
        proto.version = schema.version
    if schema.content_type is not None:
        proto.content_type = schema.content_type
    if schema.descriptor_ref is not None:
        proto.descriptor_ref = schema.descriptor_ref
    if schema.hash is not None:
        proto.hash = schema.hash
    return proto


def _from_proto_schema(schema) -> MessageBodySchema:
    return MessageBodySchema(
        schema.name or None,
        schema.version or None,
        schema.content_type or None,
        schema.descriptor_ref or None,
        schema.hash or None,
    )


def _to_telemetry(body):
    obj = body if isinstance(body, dict) else {}
    proto = _new("SouthboundSignalUpdate")
    signal = obj.get("signal")
    if isinstance(signal, dict):
        proto.signal.CopyFrom(_to_signal(signal))
    for sample in obj.get("samples", []) if isinstance(obj.get("samples"), list) else []:
        if isinstance(sample, dict):
            proto.samples.add().CopyFrom(_to_sample(sample))
    _copy_extra(obj, proto.extra, ("signal", "samples"))
    return proto


def _to_signal(obj: dict):
    proto = _new("Signal")
    if "id" in obj:
        proto.id = str(obj["id"])
    if "name" in obj:
        proto.name = str(obj["name"])
    if "address" in obj:
        proto.address.CopyFrom(_to_ec_value(obj["address"]))
    _copy_extra(obj, proto.extra, ("id", "name", "address"))
    return proto


def _to_sample(obj: dict):
    proto = _new("Sample")
    if "value" in obj:
        proto.value.CopyFrom(_to_ec_value(obj["value"]))
    if "quality" in obj:
        proto.quality = str(obj["quality"])
    if "qualityRaw" in obj:
        proto.quality_raw.CopyFrom(_to_ec_value(obj["qualityRaw"]))
    elif "quality_raw" in obj:
        proto.quality_raw.CopyFrom(_to_ec_value(obj["quality_raw"]))
    if "sourceTs" in obj:
        proto.source_ts = str(obj["sourceTs"])
        parsed = _parse_epoch_millis(obj["sourceTs"])
        if parsed is not None:
            proto.source_ts_ms = parsed
    elif "source_ts" in obj:
        proto.source_ts = str(obj["source_ts"])
    if "sourceTsMs" in obj:
        proto.source_ts_ms = int(obj["sourceTsMs"])
    elif "source_ts_ms" in obj:
        proto.source_ts_ms = int(obj["source_ts_ms"])
    if "serverTs" in obj:
        proto.server_ts = str(obj["serverTs"])
        parsed = _parse_epoch_millis(obj["serverTs"])
        if parsed is not None:
            proto.server_ts_ms = parsed
    elif "server_ts" in obj:
        proto.server_ts = str(obj["server_ts"])
    if "serverTsMs" in obj:
        proto.server_ts_ms = int(obj["serverTsMs"])
    elif "server_ts_ms" in obj:
        proto.server_ts_ms = int(obj["server_ts_ms"])
    _copy_extra(
        obj,
        proto.extra,
        (
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
        ),
    )
    return proto


def _from_telemetry(proto) -> dict:
    obj = {}
    if proto.HasField("signal"):
        signal = {"id": proto.signal.id}
        if proto.signal.name:
            signal["name"] = proto.signal.name
        if proto.signal.HasField("address"):
            signal["address"] = _from_ec_value(proto.signal.address)
        for key, value in proto.signal.extra.items():
            signal[key] = _from_ec_value(value)
        obj["signal"] = signal
    samples = []
    for sample in proto.samples:
        item = {}
        if sample.HasField("value"):
            item["value"] = _from_ec_value(sample.value)
        if sample.quality:
            item["quality"] = sample.quality
        if sample.HasField("quality_raw"):
            item["qualityRaw"] = _from_ec_value(sample.quality_raw)
        if sample.HasField("source_ts"):
            item["sourceTs"] = sample.source_ts
        if sample.HasField("source_ts_ms"):
            item["sourceTsMs"] = sample.source_ts_ms
        if sample.HasField("server_ts"):
            item["serverTs"] = sample.server_ts
        if sample.HasField("server_ts_ms"):
            item["serverTsMs"] = sample.server_ts_ms
        for key, value in sample.extra.items():
            item[key] = _from_ec_value(value)
        samples.append(item)
    obj["samples"] = samples
    for key, value in proto.extra.items():
        obj[key] = _from_ec_value(value)
    return obj


def _to_config_update(body):
    proto = _new("ConfigUpdate")
    obj = body if isinstance(body, dict) else {}
    if "config" in obj:
        proto.config.CopyFrom(_to_ec_value(obj["config"]))
    else:
        proto.config.CopyFrom(_to_ec_value(obj))
    _copy_extra(obj, proto.extra, ("config",))
    return proto


def _from_config_update(proto) -> dict:
    obj = {}
    if proto.HasField("config"):
        obj["config"] = _from_ec_value(proto.config)
    for key, value in proto.extra.items():
        obj[key] = _from_ec_value(value)
    return obj


def _to_state_update(body):
    proto = _new("StateUpdate")
    obj = body if isinstance(body, dict) else {}
    if "status" in obj:
        proto.status = str(obj["status"])
    if "uptimeSecs" in obj:
        proto.uptime_secs = int(obj["uptimeSecs"])
    elif "uptime_secs" in obj:
        proto.uptime_secs = int(obj["uptime_secs"])
    for item in obj.get("instances", []) if isinstance(obj.get("instances"), list) else []:
        if isinstance(item, dict):
            inst = proto.instances.add()
            if "instance" in item:
                inst.instance = str(item["instance"])
            if "connected" in item:
                inst.connected = bool(item["connected"])
            if "detail" in item:
                inst.detail = str(item["detail"])
            _copy_extra(item, inst.extra, ("instance", "connected", "detail"))
    _copy_extra(obj, proto.extra, ("status", "uptimeSecs", "uptime_secs", "instances"))
    return proto


def _from_state_update(proto) -> dict:
    obj = {}
    if proto.status:
        obj["status"] = proto.status
    if proto.HasField("uptime_secs"):
        obj["uptimeSecs"] = proto.uptime_secs
    if proto.instances:
        instances = []
        for item in proto.instances:
            inst = {"instance": item.instance, "connected": item.connected}
            if item.HasField("detail"):
                inst["detail"] = item.detail
            for key, value in item.extra.items():
                inst[key] = _from_ec_value(value)
            instances.append(inst)
        obj["instances"] = instances
    for key, value in proto.extra.items():
        obj[key] = _from_ec_value(value)
    return obj


def _to_metric_update(body):
    proto = _new("MetricUpdate")
    obj = body if isinstance(body, dict) else {}
    if "namespace" in obj:
        proto.namespace = str(obj["namespace"])
    if "metricName" in obj:
        proto.metric_name = str(obj["metricName"])
    elif "metric_name" in obj:
        proto.metric_name = str(obj["metric_name"])
    if "timestampMs" in obj:
        proto.timestamp_ms = int(obj["timestampMs"])
    elif "timestamp_ms" in obj:
        proto.timestamp_ms = int(obj["timestamp_ms"])
    dims = obj.get("dimensions")
    if isinstance(dims, dict):
        for key, value in dims.items():
            proto.dimensions[str(key)] = str(value)
    for item in obj.get("values", []) if isinstance(obj.get("values"), list) else []:
        if isinstance(item, dict):
            value = proto.values.add()
            if "name" in item:
                value.name = str(item["name"])
            if "value" in item:
                value.value = float(item["value"])
            if "unit" in item:
                value.unit = str(item["unit"])
            if "storageResolution" in item:
                value.storage_resolution = int(item["storageResolution"])
            elif "storage_resolution" in item:
                value.storage_resolution = int(item["storage_resolution"])
    if "largeFleetWorkaround" in obj:
        proto.large_fleet_workaround = bool(obj["largeFleetWorkaround"])
    elif "large_fleet_workaround" in obj:
        proto.large_fleet_workaround = bool(obj["large_fleet_workaround"])
    if "emfProjection" in obj:
        proto.emf_projection.CopyFrom(_to_ec_value(obj["emfProjection"]))
    elif "emf_projection" in obj:
        proto.emf_projection.CopyFrom(_to_ec_value(obj["emf_projection"]))
    _copy_extra(
        obj,
        proto.extra,
        (
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
        ),
    )
    return proto


def _from_metric_update(proto) -> dict:
    obj = {}
    if proto.namespace:
        obj["namespace"] = proto.namespace
    if proto.metric_name:
        obj["metricName"] = proto.metric_name
    if proto.timestamp_ms:
        obj["timestampMs"] = proto.timestamp_ms
    if proto.dimensions:
        obj["dimensions"] = dict(proto.dimensions)
    if proto.values:
        values = []
        for value in proto.values:
            item = {"value": value.value}
            if value.name:
                item["name"] = value.name
            if value.unit:
                item["unit"] = value.unit
            if value.storage_resolution:
                item["storageResolution"] = value.storage_resolution
            values.append(item)
        obj["values"] = values
    if proto.large_fleet_workaround:
        obj["largeFleetWorkaround"] = True
    if proto.HasField("emf_projection"):
        obj["emfProjection"] = _from_ec_value(proto.emf_projection)
    for key, value in proto.extra.items():
        obj[key] = _from_ec_value(value)
    return obj


def _to_event_message(body):
    proto = _new("EventMessage")
    obj = body if isinstance(body, dict) else {}
    if "severity" in obj:
        proto.severity = str(obj["severity"])
    if "type" in obj:
        proto.type = str(obj["type"])
    if "message" in obj:
        proto.message = str(obj["message"])
    if "timestamp" in obj:
        proto.timestamp = str(obj["timestamp"])
    if "timestampMs" in obj:
        proto.timestamp_ms = int(obj["timestampMs"])
    elif "timestamp_ms" in obj:
        proto.timestamp_ms = int(obj["timestamp_ms"])
    if "context" in obj:
        proto.context.CopyFrom(_to_ec_value(obj["context"]))
    if "alarm" in obj:
        proto.alarm = bool(obj["alarm"])
    if "active" in obj:
        proto.active = bool(obj["active"])
    _copy_extra(obj, proto.extra, ("severity", "type", "message", "timestamp", "timestampMs", "timestamp_ms", "context", "alarm", "active"))
    return proto


def _from_event_message(proto) -> dict:
    obj = {}
    if proto.severity:
        obj["severity"] = proto.severity
    if proto.type:
        obj["type"] = proto.type
    if proto.HasField("message"):
        obj["message"] = proto.message
    if proto.timestamp:
        obj["timestamp"] = proto.timestamp
    if proto.HasField("timestamp_ms"):
        obj["timestampMs"] = proto.timestamp_ms
    if proto.HasField("context"):
        obj["context"] = _from_ec_value(proto.context)
    if proto.HasField("alarm"):
        obj["alarm"] = proto.alarm
    if proto.HasField("active"):
        obj["active"] = proto.active
    for key, value in proto.extra.items():
        obj[key] = _from_ec_value(value)
    return obj


def _to_command_message(header_name: str, body):
    proto = _new("CommandMessage")
    obj = body if isinstance(body, dict) else {}
    proto.verb = str(obj.get("verb", header_name))
    wrapped_payload = False
    if "payload" in obj:
        proto.payload.CopyFrom(_to_ec_value(obj["payload"]))
    elif not any(key in obj for key in ("ok", "result", "error")):
        proto.payload.CopyFrom(_to_ec_value(obj))
        wrapped_payload = True
    if "ok" in obj:
        proto.ok = bool(obj["ok"])
    if "result" in obj:
        proto.result.CopyFrom(_to_ec_value(obj["result"]))
    if isinstance(obj.get("error"), dict):
        error = obj["error"]
        if "code" in error:
            proto.error.code = str(error["code"])
        if "message" in error:
            proto.error.message = str(error["message"])
        details = error.get("details")
        if isinstance(details, dict):
            for key, value in details.items():
                proto.error.details[str(key)].CopyFrom(_to_ec_value(value))
    if not wrapped_payload:
        _copy_extra(obj, proto.extra, ("verb", "payload", "ok", "result", "error"))
    return proto


def _from_command_message(proto) -> dict:
    if (
        proto.HasField("payload")
        and not proto.HasField("ok")
        and not proto.HasField("result")
        and not proto.HasField("error")
        and not proto.extra
    ):
        payload = _from_ec_value(proto.payload)
        return payload if isinstance(payload, dict) else {}
    obj = {}
    if proto.verb:
        obj["verb"] = proto.verb
    if proto.HasField("payload"):
        obj["payload"] = _from_ec_value(proto.payload)
    if proto.HasField("ok"):
        obj["ok"] = proto.ok
    if proto.HasField("result"):
        obj["result"] = _from_ec_value(proto.result)
    if proto.HasField("error"):
        error = {}
        if proto.error.code:
            error["code"] = proto.error.code
        if proto.error.message:
            error["message"] = proto.error.message
        if proto.error.details:
            error["details"] = {key: _from_ec_value(value) for key, value in proto.error.details.items()}
        obj["error"] = error
    for key, value in proto.extra.items():
        obj[key] = _from_ec_value(value)
    return obj


def _to_ec_value(value):
    proto = _new("EcValue")
    from edgecommons.messaging.message import BINARY_BODY_KEY, _decode_binary_descriptor

    if value is None:
        proto.null_value = 0
    elif isinstance(value, bool):
        proto.bool_value = value
    elif isinstance(value, int) and not isinstance(value, bool):
        proto.int_value = value
    elif isinstance(value, float):
        if math.isnan(value) or math.isinf(value):
            raise ValueError("EdgeCommons protobuf structured values reject NaN and infinity")
        proto.double_value = value
    elif isinstance(value, str):
        proto.string_value = value
    elif isinstance(value, (bytes, bytearray)):
        proto.bytes_value = bytes(value)
    elif isinstance(value, dict):
        marker = value.get(BINARY_BODY_KEY)
        if isinstance(marker, dict):
            proto.bytes_value = _decode_binary_descriptor(marker)
        else:
            proto.map_value.SetInParent()
            for key, item in value.items():
                proto.map_value.fields[str(key)].CopyFrom(_to_ec_value(item))
    elif isinstance(value, (list, tuple)):
        proto.list_value.SetInParent()
        for item in value:
            proto.list_value.values.add().CopyFrom(_to_ec_value(item))
    else:
        proto.string_value = str(value)
    return proto


def _from_ec_value(value):
    from edgecommons.messaging.message import _binary_marker

    selected = value.WhichOneof("kind")
    if selected is None or selected == "null_value":
        return None
    if selected == "bool_value":
        return value.bool_value
    if selected == "int_value":
        return value.int_value
    if selected == "uint_value":
        return value.uint_value
    if selected == "double_value":
        return value.double_value
    if selected == "string_value":
        return value.string_value
    if selected == "bytes_value":
        return _binary_marker(bytes(value.bytes_value))
    if selected == "list_value":
        return [_from_ec_value(item) for item in value.list_value.values]
    if selected == "map_value":
        return {key: _from_ec_value(item) for key, item in value.map_value.fields.items()}
    return None


def _copy_extra(source: dict, target_map, known_keys: Iterable[str]) -> None:
    known = set(known_keys)
    for key, value in source.items():
        if key not in known:
            target_map[key].CopyFrom(_to_ec_value(value))


@lru_cache(maxsize=1)
def _classes() -> Dict[str, Any]:
    pool = descriptor_pool.DescriptorPool()
    fds = descriptor_pb2.FileDescriptorSet()
    fds.ParseFromString(_read_descriptor_bytes())
    for file_proto in fds.file:
        pool.Add(file_proto)
    names = [
        "EdgeCommonsMessage",
        "Header",
        "Identity",
        "BodySchema",
        "EcValue",
        "SouthboundSignalUpdate",
        "Signal",
        "Sample",
        "StateUpdate",
        "ConfigUpdate",
        "MetricUpdate",
        "EventMessage",
        "CommandMessage",
    ]
    return {
        name: message_factory.GetMessageClass(
            pool.FindMessageTypeByName(f"edgecommons.v1.{name}")
        )
        for name in names
    }


def _new(name: str):
    return _classes()[name]()


def _read_descriptor_bytes() -> bytes:
    resource = resources.files("edgecommons").joinpath("resources/protobuf/edgecommons-v1.desc")
    if resource.is_file():
        return resource.read_bytes()
    for candidate in _descriptor_candidates():
        if candidate.is_file():
            return candidate.read_bytes()
    raise FileNotFoundError("edgecommons-v1.desc is required for protobuf messaging")


def _descriptor_candidates():
    here = Path(__file__).resolve()
    yield here.parents[4] / "protobuf-test-vectors" / "edgecommons-v1.desc"
    yield here.parents[3] / "protobuf-test-vectors" / "edgecommons-v1.desc"


def _coerce_body_case(value) -> MessageBodyCase:
    if isinstance(value, MessageBodyCase):
        return value
    return MessageBodyCase[str(value)] if str(value) in MessageBodyCase.__members__ else MessageBodyCase(str(value))


def _content_type_or_default(value: Optional[str]) -> str:
    return value if value else DEFAULT_OPAQUE_CONTENT_TYPE


def _string_or_none(src: dict, key: str) -> Optional[str]:
    value = src.get(key)
    return value if isinstance(value, str) else None


def _iso_from_epoch_ms(timestamp_ms: int) -> str:
    return datetime.fromtimestamp(timestamp_ms / 1000, timezone.utc).isoformat().replace("+00:00", "Z")


def _parse_epoch_millis(value: Any) -> Optional[int]:
    if not isinstance(value, str):
        return None
    try:
        normalized = value.replace("Z", "+00:00")
        return int(datetime.fromisoformat(normalized).timestamp() * 1000)
    except ValueError:
        return None


def _snake_case(value: str) -> str:
    out = []
    for ch in value:
        if ch.isupper():
            out.append("_")
            out.append(ch.lower())
        else:
            out.append(ch)
    return "".join(out).lstrip("_")
