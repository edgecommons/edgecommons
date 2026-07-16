import threading
from pathlib import Path

import pytest

from edgecommons.messaging.message import Message, _binary_marker, _decode_binary_descriptor
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.proto import MessageBodyCase, MessageBodySchema
from edgecommons.messaging import proto as message_proto
from edgecommons.messaging.qos import Qos
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider

import edgecommons.messaging.providers.standalone_provider as sp
from tests.test_standalone_provider_mock import FakeMqttClient, _local_only_config


@pytest.fixture(autouse=True)
def _patch_paho(monkeypatch):
    FakeMqttClient.instances = []
    monkeypatch.setattr(sp.mqtt, "Client", FakeMqttClient)
    yield


def test_structured_body_round_trips_through_protobuf_bytes():
    payload = {"temperature": 21.5, "ok": True, "nested": {"line": "A"}}
    message = (
        MessageBuilder.create("StructuredSample", "1.0")
        .with_timestamp_ms(1783360800000)
        .with_uuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f1155")
        .with_structured_payload(payload)
        .with_tags({"retention": "short", "priority": 5})
        .build()
    )

    encoded = message.to_bytes()
    decoded = Message.from_bytes(encoded)

    assert not encoded.startswith(b"{")
    assert decoded.get_body_case() is MessageBodyCase.STRUCTURED
    assert decoded.get_body() == payload
    assert decoded.get_tags().to_dict() == {"retention": "short", "priority": 5}


def test_identity_component_scope_omits_instance_through_protobuf():
    # D-U28: a component-scope identity carries no instance; the proto codec omits the
    # instance field and parses an empty proto instance back to None.
    from edgecommons.messaging.identity import HierEntry, MessageIdentity

    identity = MessageIdentity([HierEntry("device", "gw-01")], "adapter")  # component scope
    message = (
        MessageBuilder.create("S", "1.0")
        .with_identity(identity)
        .with_structured_payload({"v": 1})
        .build()
    )
    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(message.to_bytes())
    assert proto.identity.instance == ""  # omitted on the wire
    decoded = Message.from_bytes(message.to_bytes())
    assert decoded.get_identity().instance is None
    assert "instance" not in decoded.get_identity().to_dict()


def test_identity_instance_scope_survives_protobuf():
    # A present instance token round-trips unchanged.
    from edgecommons.messaging.identity import HierEntry, MessageIdentity

    identity = MessageIdentity([HierEntry("device", "gw-01")], "adapter", "kep1")
    message = (
        MessageBuilder.create("S", "1.0")
        .with_identity(identity)
        .with_structured_payload({"v": 1})
        .build()
    )
    decoded = Message.from_bytes(message.to_bytes())
    assert decoded.get_identity().instance == "kep1"


def test_structured_body_preserves_empty_containers():
    payload = {"emptyList": [], "emptyMap": {}, "nested": {"items": []}}

    decoded = Message.from_bytes(
        MessageBuilder.create("StructuredEmpty", "1.0")
        .with_structured_payload(payload)
        .build()
        .to_bytes()
    )

    assert decoded.get_body() == payload


def test_southbound_signal_update_preserves_byte_sample():
    sample_bytes = bytes([0, 1, 2, 254, 255])
    body = {
        "signal": {
            "id": "camera-1/roi-17/thumbnail",
            "name": "Thumbnail",
            "address": {"ns": 2, "nodeId": "Line1.Thumbnail"},
        },
        "samples": [
            {
                "value": _binary_marker(sample_bytes),
                "quality": "GOOD",
                "sourceTs": "2026-07-06T17:59:59.900Z",
                "serverTs": "2026-07-06T18:00:00Z",
            }
        ],
    }

    message = (
        MessageBuilder.create("SouthboundSignalUpdate", "1.0")
        .with_timestamp_ms(1783360800000)
        .with_uuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f1155")
        .with_payload(body)
        .build()
    )

    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(message.to_bytes())

    assert proto.WhichOneof("body") == "southbound_signal_update"
    sample = proto.southbound_signal_update.samples[0]
    assert bytes(sample.value.bytes_value) == sample_bytes
    assert sample.source_ts_ms == 1783360799900
    assert (
        proto.southbound_signal_update.signal.address.map_value.fields["nodeId"].string_value
        == "Line1.Thumbnail"
    )

    decoded = Message.from_bytes(proto.SerializeToString())
    decoded_sample = decoded.get_body()["samples"][0]
    assert _decode_binary_descriptor(decoded_sample["value"]["_edgecommonsBinary"]) == sample_bytes
    assert decoded.get_body_case() is MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE


def test_opaque_body_round_trips_with_content_type_and_schema():
    jpeg_like = bytes([0xFF, 0xD8, 0xFF, 0xE0, 1, 2])
    schema = MessageBodySchema(
        "FramePreview", "1.0", "image/jpeg", "s3://descriptors/app.desc", "sha256:test"
    )

    message = (
        MessageBuilder.create("FramePreview", "1.0")
        .with_timestamp_ms(1783360800000)
        .with_uuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f1156")
        .with_opaque_payload(jpeg_like, "image/jpeg")
        .with_schema(schema)
        .with_tags({"capture_mode": "preview"})
        .build()
    )

    decoded = Message.from_bytes(message.to_bytes())

    assert decoded.get_body_case() is MessageBodyCase.OPAQUE
    assert decoded.get_content_type() == "image/jpeg"
    assert decoded.get_schema().descriptor_ref == "s3://descriptors/app.desc"
    assert decoded.get_opaque_body() == jpeg_like
    assert decoded.get_tags().to_dict()["capture_mode"] == "preview"

    diagnostic = decoded.to_diagnostic_json()
    assert diagnostic["body_case"] == "OPAQUE"
    assert diagnostic["body"]["length"] == len(jpeg_like)
    assert "_edgecommonsBinary" not in diagnostic["body"]


def test_byte_payload_defaults_to_opaque_octet_stream():
    payload = bytes([10, 20, 30])
    decoded = Message.from_bytes(
        MessageBuilder.create("OpaqueDefault", "1.0").with_payload(payload).build().to_bytes()
    )

    assert decoded.get_body_case() is MessageBodyCase.OPAQUE
    assert decoded.get_content_type() == "application/octet-stream"
    assert decoded.get_opaque_body() == payload


def test_reserved_names_infer_typed_bodies():
    state = MessageBuilder.create("state", "1.0").with_payload({"status": "RUNNING", "uptimeSecs": 42}).build()
    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(state.to_bytes())
    assert proto.WhichOneof("body") == "state_update"
    assert proto.state_update.uptime_secs == 42
    assert Message.from_bytes(proto.SerializeToString()).get_body_case() is MessageBodyCase.STATE_UPDATE

    cfg = MessageBuilder.create("cfg", "1.0").with_payload({"config": {"mode": "auto"}}).build()
    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(cfg.to_bytes())
    assert proto.WhichOneof("body") == "config_update"
    assert proto.config_update.config.map_value.fields["mode"].string_value == "auto"

    metric = (
        MessageBuilder.create("Metric", "1.0")
        .with_payload({
            "namespace": "EdgeCommons",
            "metricName": "MessagesPublished",
            "values": [{"name": "Count", "value": 3.0, "unit": "Count"}],
        })
        .build()
    )
    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(metric.to_bytes())
    assert proto.WhichOneof("body") == "metric_update"
    assert proto.metric_update.metric_name == "MessagesPublished"

    event = (
        MessageBuilder.create("evt", "1.0")
        .with_payload({"severity": "info", "type": "door-open", "message": "door opened"})
        .build()
    )
    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(event.to_bytes())
    assert proto.WhichOneof("body") == "event"
    assert proto.event.type == "door-open"


def test_explicit_command_body_preserves_component_facing_payload():
    message = MessageBuilder.create("ping", "1.0").with_command({"status": "RUNNING"}).build()
    proto = message_proto._new("EdgeCommonsMessage")
    proto.ParseFromString(message.to_bytes())

    assert proto.WhichOneof("body") == "command"
    assert proto.command.verb == "ping"
    assert proto.command.payload.map_value.fields["status"].string_value == "RUNNING"
    decoded = Message.from_bytes(proto.SerializeToString())
    assert decoded.get_body_case() is MessageBodyCase.COMMAND
    assert decoded.get_body() == {"status": "RUNNING"}


def test_malformed_protobuf_is_rejected():
    with pytest.raises(ValueError, match="Malformed EdgeCommons protobuf message"):
        Message.from_bytes(b"not-json-and-not-protobuf")


def test_canonical_protobuf_vectors_round_trip_exact_bytes():
    vectors = Path(__file__).parents[3] / "protobuf-test-vectors" / "messages.pb.hex"
    assert vectors.is_file()
    for line in vectors.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        vector_id, hex_value = line.split(" ", 1)
        message = Message.from_bytes(bytes.fromhex(hex_value))
        assert message.to_bytes().hex() == hex_value, vector_id


def test_standalone_mqtt_publish_payload_is_protobuf_and_parseable():
    provider = StandaloneProvider(_local_only_config(), "thing-1")
    try:
        provider.publish("out/topic", MessageBuilder.create("Hello", "1.0").with_payload({"a": 1}).build())
        _topic, payload, qos_value = provider.get_native_client()["local"].published[0]
        proto = message_proto._new("EdgeCommonsMessage")
        proto.ParseFromString(payload)
        assert proto.header.name == "Hello"
        assert proto.WhichOneof("body") == "structured"
        assert qos_value == Qos.AT_LEAST_ONCE.mqtt_level
    finally:
        provider.disconnect()


def test_raw_payload_is_not_delivered_to_message_subscription():
    provider = StandaloneProvider(_local_only_config(), "thing-1")
    delivered = threading.Event()
    try:
        provider.subscribe("raw/topic", lambda _topic, _msg: delivered.set())
        provider.get_native_client()["local"].deliver("raw/topic", b'{"raw": true}')
        assert not delivered.wait(0.2)
    finally:
        provider.disconnect()
