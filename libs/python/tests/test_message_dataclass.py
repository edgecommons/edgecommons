"""Tests for the Message/MessageHeader/MessageTags dataclasses: serialization is
byte-for-byte unchanged and the computed-default behavior is preserved."""
import json

from edgecommons.messaging.message import Message, MessageHeader, MessageTags


def test_header_computed_defaults_and_explicit_values():
    h = MessageHeader("N", "1.0")
    assert h.timestamp and h.correlation_id and h.uuid  # auto-filled when omitted

    h2 = MessageHeader("N", "1.0", correlation_id="cid", timestamp="ts", uuid="u", reply_to="r")
    assert (h2.correlation_id, h2.timestamp, h2.uuid, h2.reply_to) == ("cid", "ts", "u", "r")


def test_header_to_dict_and_roundtrip():
    h = MessageHeader("N", "1.0", correlation_id="cid", timestamp="ts", uuid="u", reply_to="r")
    assert h.to_dict() == {
        "name": "N", "version": "1.0", "timestamp": "ts", "uuid": "u",
        "correlation_id": "cid", "reply_to": "r",
    }
    assert MessageHeader.from_dict(h.to_dict()) == h  # dataclass __eq__

    # reply_to is omitted when None.
    h3 = MessageHeader("N", "1.0", correlation_id="c", timestamp="t", uuid="u")
    assert "reply_to" not in h3.to_dict()


def test_tags_have_no_thing_special_casing():
    # UNS hard cut (§1.1): tags.thing is removed — the device travels in the
    # top-level identity element; a stray "thing" key is just a generic tag.
    assert MessageTags({"a": "b"}).to_dict() == {"a": "b"}
    assert MessageTags.from_dict({"a": "b", "thing": "t"}).to_dict() == {"a": "b", "thing": "t"}
    assert MessageTags(None).tags == {}  # default tags


def test_message_serialization_shape():
    m = Message()
    m.header = MessageHeader("N", "1.0", correlation_id="c", timestamp="t", uuid="u")
    m.tags = MessageTags({"site": "s"})
    m.body = {"x": 1}
    d = json.loads(m.dumps())
    assert set(d.keys()) == {"header", "tags", "body"}
    assert d["body"] == {"x": 1}
    assert d["tags"] == {"site": "s"}
    assert d["header"]["name"] == "N"


def test_message_raw_serialization():
    m = Message()
    m.raw = "raw-bytes"
    assert m.to_dict() == {"raw": "raw-bytes"}


def test_from_object_roundtrip():
    src = {
        "header": {"name": "N", "version": "1.0", "correlation_id": "c",
                   "timestamp": "t", "uuid": "u"},
        "tags": {"site": "s"},
        "body": {"x": 1},
    }
    m = Message.from_object(src)
    assert m.get_body() == {"x": 1}
    assert m.get_tags().tags == {"site": "s"}
    assert m.get_header().name == "N"


def test_from_object_non_envelope_dict_is_raw():
    # A JSON object with none of header/tags/body is a raw message (parity with
    # Java getRaw()/Rust is_raw()), not an envelope body.
    m = Message.from_object({"temperature": 21.5})
    assert m.get_raw() == {"temperature": 21.5}
    assert m.get_body() is None
    assert m.to_dict() == {"raw": {"temperature": 21.5}}


def test_from_object_non_dict_is_raw():
    m = Message.from_object("hello")
    assert m.get_raw() == "hello"
    assert m.get_body() is None


def test_message_equality():
    a = Message()
    a.body = {"x": 1}
    b = Message()
    b.body = {"x": 1}
    assert a == b  # dataclass-generated __eq__
