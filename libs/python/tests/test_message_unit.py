"""Unit tests for Message / MessageHeader / MessageTags and MessageBuilder."""
import json

import pytest

from edgecommons.messaging.message import MAX_BINARY_BODY_BYTES, Message, MessageHeader, MessageTags
from edgecommons.messaging.message_builder import MessageBuilder


class TestMessageHeader:
    def test_defaults_filled(self):
        h = MessageHeader("Name", "1.0")
        assert h.correlation_id is not None
        assert h.uuid is not None
        assert h.timestamp is not None

    def test_to_dict_omits_reply_to_when_absent(self):
        h = MessageHeader("Name", "1.0")
        d = h.to_dict()
        assert "reply_to" not in d
        assert d["name"] == "Name" and d["version"] == "1.0"

    def test_to_dict_includes_reply_to_when_set(self):
        h = MessageHeader("Name", "1.0")
        h.make_request("reply/here")
        assert h.get_reply_to() == "reply/here"
        assert h.to_dict()["reply_to"] == "reply/here"

    def test_make_request_generates_default(self):
        h = MessageHeader("Name", "1.0")
        rt = h.make_request()
        assert rt.startswith(MessageHeader.REPLY_MESSAGE_TOPIC_PREFIX)

    def test_from_dict_roundtrip(self):
        src = {"name": "N", "version": "2", "correlation_id": "c", "uuid": "u",
               "timestamp": "t", "reply_to": "r"}
        h = MessageHeader.from_dict(src)
        assert h.name == "N" and h.correlation_id == "c" and h.reply_to == "r"

    def test_set_correlation_id(self):
        h = MessageHeader("N", "1")
        h.set_correlation_id("xyz")
        assert h.correlation_id == "xyz"


class TestMessageTags:
    def test_default_tags_empty(self):
        t = MessageTags()
        assert t.tags == {}

    def test_to_dict_returns_generic_map(self):
        t = MessageTags({"env": "prod"})
        assert t.to_dict() == {"env": "prod"}

    def test_no_thing_special_casing(self):
        # UNS hard cut (§1.1): tags.thing is removed — the device travels in the
        # top-level identity element. A stray inbound "thing" key just lands in the
        # generic tag map (no legacy shim).
        t = MessageTags.from_dict({"thing": "thing-1", "a": "b"})
        assert t.tags == {"thing": "thing-1", "a": "b"}
        assert not hasattr(t, "thing_name")

    def test_inject_tag(self):
        t = MessageTags()
        t.inject_tag("k", "v")
        assert t.tags["k"] == "v"


class TestMessage:
    def test_to_dict_structured(self):
        m = Message()
        m.header = MessageHeader("N", "1")
        m.tags = MessageTags({"a": "b"})
        m.body = {"v": 1}
        d = m.to_dict()
        assert d["header"]["name"] == "N"
        assert d["tags"] == {"a": "b"}
        assert d["body"] == {"v": 1}

    def test_to_dict_raw(self):
        m = Message()
        m.raw = "raw-string"
        assert m.to_dict() == {"raw": "raw-string"}

    def test_bytes_body_serializes_as_binary_marker(self):
        m = Message()
        m.body = bytes([0, 1, 2, 254, 255])
        d = m.to_dict()
        assert d["body"] == {
            "_edgecommonsBinary": {
                "encoding": "base64",
                "length": 5,
                "data": "AAEC/v8=",
            }
        }
        assert json.loads(str(m))["body"]["_edgecommonsBinary"]["data"] == "AAEC/v8="
        assert m.is_binary_body()
        assert m.get_binary_body() == bytes([0, 1, 2, 254, 255])

    def test_binary_marker_decodes_and_validates_length(self):
        m = Message.from_object({
            "body": {
                "_edgecommonsBinary": {
                    "encoding": "base64",
                    "length": 5,
                    "data": "AAEC/v8=",
                }
            }
        })
        assert m.is_binary_body()
        assert m.get_binary_body() == bytes([0, 1, 2, 254, 255])
        m.body["_edgecommonsBinary"]["length"] = 4
        with pytest.raises(ValueError):
            m.get_binary_body()

    def test_oversized_binary_body_is_rejected(self):
        m = Message()
        m.body = bytes(MAX_BINARY_BODY_BYTES + 1)
        with pytest.raises(ValueError):
            m.to_dict()

    def test_to_dict_preserves_explicit_null_map_entry(self):
        # #15 parity: an explicit None entry in a dict body round-trips as JSON null (Python already
        # did this; asserted here as the cross-language contract alongside the Java fix).
        m = Message()
        m.body = {"present": 1, "nullv": None}
        assert json.loads(str(m))["body"] == {"present": 1, "nullv": None}

    def test_str_serializes(self):
        m = Message()
        m.body = {"a": 1}
        assert json.loads(str(m))["body"] == {"a": 1}

    def test_dumps_with_indent(self):
        m = Message()
        m.header = MessageHeader("N", "1")
        m.body = {"a": 1}
        out = m.dumps(indent=2)
        assert "\n" in out and json.loads(out)["body"] == {"a": 1}

    def test_get_correlation_id_none_when_no_header(self):
        assert Message().get_correlation_id() is None

    def test_get_correlation_id_from_header(self):
        m = Message()
        m.header = MessageHeader("N", "1", correlation_id="c1")
        assert m.get_correlation_id() == "c1"

    def test_get_source_alias(self):
        m = Message()
        m.tags = MessageTags()
        assert m.get_source() is m.get_tags()

    def test_get_payload_alias(self):
        m = Message()
        m.body = {"p": 1}
        assert m.get_payload() == {"p": 1}

    def test_get_raw(self):
        m = Message()
        m.raw = "r"
        assert m.get_raw() == "r"

    def test_inject_tag_creates_tags(self):
        m = Message()
        m.inject_tag("k", "v")
        assert m.get_tags().tags["k"] == "v"

    def test_make_request_creates_header_when_missing(self):
        m = Message()
        rt = m.make_request("reply/x")
        assert rt == "reply/x"
        assert m.header is not None

    def test_set_correlation_id_creates_header_when_missing(self):
        m = Message()
        m.set_correlation_id("c9")
        assert m.get_correlation_id() == "c9"

    def test_set_correlation_id_updates_existing_header(self):
        m = Message()
        m.header = MessageHeader("N", "1")
        m.set_correlation_id("c10")
        assert m.header.correlation_id == "c10"

    def test_from_object_envelope(self):
        m = Message.from_object({"header": {"name": "N", "version": "1"}, "tags": {"site": "s1"}, "body": {"v": 1}})
        assert m.header.name == "N"
        assert m.tags.tags == {"site": "s1"}
        assert m.body == {"v": 1}

    def test_from_object_raw_dict(self):
        m = Message.from_object({"x": 1, "y": 2})
        assert m.raw == {"x": 1, "y": 2}

    def test_from_object_non_dict(self):
        m = Message.from_object("just-a-string")
        assert m.raw == "just-a-string"


class TestMessageBuilder:
    def test_build_without_config_omits_identity_and_tags(self):
        # Bootstrap/raw messages legally omit identity and tags (UNS §1.4): a builder
        # with neither a config service nor tags builds a header+body-only envelope.
        m = MessageBuilder.create("N", "1").with_payload({"a": 1}).build()
        assert m.identity is None and m.tags is None
        d = m.to_dict()
        assert "identity" not in d and "tags" not in d

    def test_build_with_tags_and_dict_payload(self):
        m = MessageBuilder.create("N", "1").with_payload({"a": 1}).with_tags({"env": "t"}).build()
        assert m.body == {"a": 1}
        assert m.tags.tags == {"env": "t"}
        assert m.header.name == "N"

    def test_build_with_str_json_payload_parsed(self):
        m = MessageBuilder.create("N", "1").with_payload('{"a": 1}').with_tags({}).build()
        assert m.body == {"a": 1}

    def test_build_with_str_non_json_payload_kept(self):
        m = MessageBuilder.create("N", "1").with_payload("not-json").with_tags({}).build()
        assert m.body == "not-json"

    def test_builder_setters(self):
        m = (
            MessageBuilder.create("N", "1")
            .with_correlation_id("c")
            .with_uuid("u")
            .with_timestamp("ts")
            .with_reply_to("r")
            .with_payload({})
            .with_tags({})
            .build()
        )
        assert m.header.correlation_id == "c"
        assert m.header.uuid == "u"
        assert m.header.timestamp == "ts"
        assert m.header.reply_to == "r"

    def test_from_object_with_header(self):
        builder = MessageBuilder.from_object({
            "header": {"name": "N", "version": "2", "correlation_id": "c", "uuid": "u",
                       "timestamp": "ts", "reply_to": "r"},
            "body": {"v": 1},
            "tags": {"env": "t"},
        })
        m = builder.build()
        assert m.header.name == "N" and m.header.version == "2"
        assert m.header.correlation_id == "c"
        assert m.body == {"v": 1}
        assert m.tags.tags == {"env": "t"}

    def test_from_object_raw(self):
        builder = MessageBuilder.from_object("raw-payload")
        m = builder.with_tags({}).build()
        assert m.header.name == "raw"
        assert m.body == "raw-payload"
