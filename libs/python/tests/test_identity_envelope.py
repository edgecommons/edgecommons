"""Unit tests for the top-level UNS ``identity`` envelope element
(UNS-CANONICAL-DESIGN §1): the MessageIdentity type, its lenient wire parser, the
envelope serialization order/detection, and the MessageBuilder stamping rules."""
import json

import pytest

from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message import Message, MessageTags
from edgecommons.messaging.message_builder import MessageBuilder


def _identity(instance=None):
    return MessageIdentity(
        [HierEntry("site", "dallas"), HierEntry("zone", "z3"), HierEntry("device", "gw-01")],
        "opcua-adapter",
        instance,
    )


class TestHierEntry:
    def test_valid(self):
        e = HierEntry("site", "dallas")
        assert e.level == "site" and e.value == "dallas"
        assert e == HierEntry("site", "dallas")
        assert e != HierEntry("site", "austin")
        assert "site" in repr(e)

    def test_empty_level_rejected(self):
        with pytest.raises(ValueError):
            HierEntry("", "v")

    def test_empty_value_rejected(self):
        with pytest.raises(ValueError):
            HierEntry("site", "")


class TestMessageIdentity:
    def test_path_precomputed_and_device_is_last_entry(self):
        ident = _identity()
        assert ident.path == "dallas/z3/gw-01"
        assert ident.device == "gw-01"
        assert ident.component == "opcua-adapter"
        assert ident.instance == "main"  # default

    def test_empty_hier_rejected(self):
        with pytest.raises(ValueError):
            MessageIdentity([], "comp")

    def test_empty_component_rejected(self):
        with pytest.raises(ValueError):
            MessageIdentity([HierEntry("device", "d")], "")

    def test_with_instance_copies(self):
        ident = _identity()
        other = ident.with_instance("kep1")
        assert other.instance == "kep1"
        assert other.hier == ident.hier and other.path == ident.path
        assert ident.instance == "main"  # original untouched

    def test_with_instance_empty_rejected(self):
        with pytest.raises(ValueError):
            _identity().with_instance("")

    def test_to_dict_canonical_order(self):
        d = _identity("kep1").to_dict()
        assert list(d.keys()) == ["hier", "path", "component", "instance"]
        assert d["hier"][0] == {"level": "site", "value": "dallas"}
        assert d["instance"] == "kep1"

    def test_equality_and_str(self):
        assert _identity() == _identity()
        assert _identity() != _identity("kep1")
        assert json.loads(str(_identity()))["path"] == "dallas/z3/gw-01"


class TestFromDictLenient:
    def test_roundtrip(self):
        ident = _identity("kep1")
        parsed = MessageIdentity.from_dict(ident.to_dict())
        assert parsed == ident

    def test_missing_instance_defaults_to_main(self):
        d = _identity().to_dict()
        del d["instance"]
        assert MessageIdentity.from_dict(d).instance == "main"

    def test_missing_path_recomputed(self):
        d = _identity().to_dict()
        del d["path"]
        assert MessageIdentity.from_dict(d).path == "dallas/z3/gw-01"

    def test_present_path_authoritative(self):
        # The publisher is authoritative: a present path is taken as-is.
        d = _identity().to_dict()
        d["path"] = "publisher/chose/this"
        assert MessageIdentity.from_dict(d).path == "publisher/chose/this"

    @pytest.mark.parametrize("bad", [
        None, "not-an-object", 42, [],                       # non-dict
        {},                                                   # no hier
        {"hier": [], "component": "c"},                       # empty hier
        {"hier": "x", "component": "c"},                      # non-array hier
        {"hier": ["x"], "component": "c"},                    # entry not an object
        {"hier": [{"level": "d"}], "component": "c"},         # entry missing value
        {"hier": [{"level": "", "value": "v"}], "component": "c"},  # empty level
        {"hier": [{"level": "d", "value": "v"}]},             # missing component
        {"hier": [{"level": "d", "value": "v"}], "component": ""},  # empty component
        {"hier": [{"level": "d", "value": 42}], "component": "c"},  # non-string value
    ])
    def test_malformed_yields_none(self, bad):
        assert MessageIdentity.from_dict(bad) is None


class TestEnvelope:
    def test_serialized_between_header_and_tags(self):
        m = (
            MessageBuilder.create("N", "1")
            .with_identity(_identity())
            .with_tags({"a": "b"})
            .with_payload({"v": 1})
            .build()
        )
        d = m.to_dict()
        assert list(d.keys()) == ["header", "identity", "tags", "body"]
        assert json.loads(m.dumps())["identity"]["path"] == "dallas/z3/gw-01"

    def test_identity_alone_is_an_envelope_marker(self):
        # Envelope detection is has-any-of header|identity|tags|body.
        m = Message.from_object({"identity": _identity().to_dict()})
        assert m.raw is None
        assert m.get_identity() == _identity()

    def test_malformed_inbound_identity_still_delivers(self):
        m = Message.from_object({
            "header": {"name": "N", "version": "1"},
            "identity": {"hier": []},
            "body": {"v": 1},
        })
        assert m.get_identity() is None  # lenient: dropped with a WARN
        assert m.get_body() == {"v": 1}

    def test_non_object_identity_still_delivers(self):
        m = Message.from_object({"identity": "bogus", "body": {"v": 1}})
        assert m.get_identity() is None
        assert m.get_body() == {"v": 1}

    def test_raw_messages_never_carry_identity(self):
        m = Message.from_object({"x": 1})
        assert m.raw == {"x": 1}
        assert "identity" not in m.to_dict()


class TestBuilderStamping:
    class _Cm:
        """Minimal config-service stand-in for the stamping rules."""

        def __init__(self, identity):
            self._identity = identity

        def get_component_identity(self):
            return self._identity

        def get_tag_config(self):
            return None

    def test_override_wins_over_config(self):
        override = MessageIdentity([HierEntry("device", "other")], "other-comp", "x")
        m = (
            MessageBuilder.create("N", "1")
            .with_config(self._Cm(_identity()))
            .with_identity(override)
            .with_instance("ignored")  # not applied to an override
            .build()
        )
        assert m.get_identity() is override

    def test_config_identity_stamped_with_default_instance(self):
        m = MessageBuilder.create("N", "1").with_config(self._Cm(_identity())).build()
        assert m.get_identity().instance == "main"
        assert m.get_identity().component == "opcua-adapter"

    def test_config_identity_stamped_with_instance_token(self):
        m = (
            MessageBuilder.create("N", "1")
            .with_config(self._Cm(_identity()))
            .with_instance("kep1")
            .build()
        )
        assert m.get_identity().instance == "kep1"

    def test_no_config_no_override_stays_none(self):
        m = MessageBuilder.create("N", "1").with_payload({"v": 1}).build()
        assert m.get_identity() is None

    def test_config_without_resolved_identity_stays_none(self):
        m = MessageBuilder.create("N", "1").with_config(self._Cm(None)).build()
        assert m.get_identity() is None
        assert m.tags is not None  # tags still stamped from config

    def test_with_instance_empty_rejected(self):
        with pytest.raises(ValueError):
            MessageBuilder.create("N", "1").with_instance("")

    def test_from_object_carries_identity(self):
        src = {
            "header": {"name": "N", "version": "1", "uuid": "u", "timestamp": "t",
                       "timestamp_ms": 0, "correlation_id": "c"},
            "identity": _identity("kep1").to_dict(),
            "body": {"v": 1},
        }
        rebuilt = MessageBuilder.from_object(src).build()
        assert rebuilt.get_identity() == _identity("kep1")
        assert rebuilt.to_dict() == src

    def test_message_tags_thing_removed(self):
        # The hard cut: MessageTags carries only the generic map.
        t = MessageTags.from_dict({"thing": "still-a-tag", "site": "s"})
        assert t.to_dict() == {"thing": "still-a-tag", "site": "s"}
        assert not hasattr(t, "thing_name")
