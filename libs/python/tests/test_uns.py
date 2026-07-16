"""Unit tests for the UNS topic builder/validator surface not pinned by the shared
vectors (test_uns_vectors.py): constructor/argument guards, the scope factories, the
instance handle, and the EdgeCommons facade accessors."""
import pytest

from edgecommons.edgecommons_instance import EdgeCommonsInstance
from edgecommons.edgecommons import EdgeCommons
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.uns import (
    RESERVED_CLASSES,
    Uns,
    UnsClass,
    UnsScope,
    UnsValidationError,
)


def _identity(instance=None):
    return MessageIdentity(
        [HierEntry("site", "dallas"), HierEntry("device", "gw-01")],
        "opcua-adapter",
        instance,
    )


class TestUnsClass:
    def test_tokens_and_leaf_semantics(self):
        assert UnsClass.STATE.token == "state" and UnsClass.STATE.leaf
        assert UnsClass.CFG.token == "cfg" and UnsClass.CFG.leaf
        for cls in (UnsClass.METRIC, UnsClass.LOG, UnsClass.DATA, UnsClass.EVT,
                    UnsClass.CMD, UnsClass.APP):
            assert not cls.leaf

    def test_reserved_set(self):
        assert RESERVED_CLASSES == {UnsClass.STATE, UnsClass.METRIC, UnsClass.CFG, UnsClass.LOG}
        assert UnsClass.RESERVED == RESERVED_CLASSES  # Java-parity alias

    def test_from_token(self):
        assert UnsClass.from_token("data") is UnsClass.DATA
        assert UnsClass.from_token("bogus") is None
        assert UnsClass.from_token("STATE") is None  # tokens are lowercase


class TestUnsScope:
    def test_factories(self):
        assert UnsScope.all() == UnsScope(None, None, None, None)
        assert UnsScope.for_device("d") == UnsScope(None, "d", None, None)
        assert UnsScope.for_component("d", "c") == UnsScope(None, "d", "c", None)
        assert UnsScope.for_instance("d", "c", "i") == UnsScope(None, "d", "c", "i")


class TestUnsConstruction:
    def test_none_identity_rejected(self):
        with pytest.raises(ValueError):
            Uns(None, False)

    def test_identity_accessor(self):
        ident = _identity()
        assert Uns(ident, False).identity() is ident

    def test_topic_for_none_target_rejected(self):
        with pytest.raises(ValueError):
            Uns(_identity(), False).topic_for(None, UnsClass.STATE)

    def test_topic_none_class_rejected(self):
        with pytest.raises(ValueError):
            Uns(_identity(), False).topic(None)

    def test_filter_none_scope_rejected(self):
        with pytest.raises(ValueError):
            Uns(_identity(), False).filter(UnsClass.DATA, None)

    def test_filter_none_class_rejected(self):
        with pytest.raises(ValueError):
            Uns(_identity(), False).filter(None, UnsScope.all())


class TestTopicFor:
    def test_addresses_a_peer_identity(self):
        # topicFor takes a peer's identity (typically a received message's) — the way
        # a component addresses a peer's cmd inbox without parsing topics.
        me = Uns(_identity(), False)
        peer = MessageIdentity([HierEntry("device", "gw-02")], "modbus-adapter", "u7")
        assert me.topic_for(peer, UnsClass.CMD, "set-log-level") == \
            "ecv1/gw-02/modbus-adapter/u7/cmd/set-log-level"

    def test_foreign_identity_with_bad_token_fails_to_build(self):
        me = Uns(_identity(), False)
        peer = MessageIdentity([HierEntry("device", "gw+02")], "comp")
        with pytest.raises(UnsValidationError) as e:
            me.topic_for(peer, UnsClass.STATE)
        assert e.value.code == UnsValidationError.BAD_CHAR


class TestCheckToken:
    def test_valid_tokens_pass(self):
        Uns.check_token("kep1", "instance id")
        Uns.check_token("with space", "token")  # spaces are legal (sanitizer parity)
        Uns.check_token("v1.2", "token")        # dots are legal

    def test_error_carries_code_and_message(self):
        with pytest.raises(UnsValidationError) as e:
            Uns.check_token("", "instance id")
        assert e.value.code == UnsValidationError.EMPTY_TOKEN
        assert "[EMPTY_TOKEN]" in str(e.value)


class TestDU28OptionalInstance:
    """D-U28: the instance slot is optional — component scope omits it; the validator
    locates the class dynamically by the class-token set; the filter can omit the
    instance slot."""

    def test_component_scope_topic_omits_instance(self):
        # _identity() is component scope (instance=None) -> no instance path segment.
        assert Uns(_identity(), False).topic(UnsClass.STATE) == "ecv1/gw-01/opcua-adapter/state"
        assert (
            Uns(_identity(), False).topic(UnsClass.DATA, "temp")
            == "ecv1/gw-01/opcua-adapter/data/temp"
        )

    def test_instance_scope_topic_keeps_instance(self):
        ident = _identity("kep1")
        assert Uns(ident, False).topic(UnsClass.STATE) == "ecv1/gw-01/opcua-adapter/kep1/state"

    def test_filter_component_scope_omits_instance_slot(self):
        uns = Uns(_identity(), False)
        assert uns.filter(UnsClass.CMD, UnsScope.all()) == "ecv1/+/+/+/cmd/#"
        assert (
            uns.filter(UnsClass.CMD, UnsScope.all(), include_instance=False)
            == "ecv1/+/+/cmd/#"
        )

    def test_validate_locates_class_for_both_scopes(self):
        uns = Uns(_identity(), False)
        uns.validate("ecv1/gw-01/opcua-adapter/state")            # component scope
        uns.validate("ecv1/gw-01/opcua-adapter/kep1/state")       # instance scope

    def test_validate_too_few_levels_is_bad_class(self):
        with pytest.raises(UnsValidationError) as e:
            Uns(_identity(), False).validate("ecv1/gw-01/opcua-adapter")
        assert e.value.code == UnsValidationError.BAD_CLASS

    def test_validate_instance_without_following_class_is_bad_class(self):
        with pytest.raises(UnsValidationError) as e:
            Uns(_identity(), False).validate("ecv1/gw-01/opcua-adapter/kep1")
        assert e.value.code == UnsValidationError.BAD_CLASS


class TestEdgeCommonsInstance:
    def _cm(self):
        class Cm:
            def get_component_identity(self):
                return _identity()

            def get_instance_ids(self):
                return ["kep1"]

            def get_tag_config(self):
                return None
        return Cm()

    def test_handle_binds_instance_into_uns_and_builder(self):
        handle = EdgeCommonsInstance("kep1", self._cm(), False)
        assert handle.id() == "kep1"
        assert handle.uns().topic(UnsClass.DATA, "temp") == \
            "ecv1/gw-01/opcua-adapter/kep1/data/temp"
        msg = handle.new_message("data", "1.0").with_payload({"v": 1}).build()
        assert msg.get_identity().instance == "kep1"
        assert msg.get_identity().component == "opcua-adapter"

    def test_include_root_flows_into_handle_uns(self):
        handle = EdgeCommonsInstance("kep1", self._cm(), True)
        assert handle.uns().topic(UnsClass.STATE) == \
            "ecv1/dallas/gw-01/opcua-adapter/kep1/state"


class TestFacadeAccessors:
    def _gg(self, identity=True, include_root=False):
        gg = object.__new__(EdgeCommons)
        gg._uns = None
        gg._instance_handles = {}
        # instance() also threads the streaming sink + clock into EdgeCommonsInstance for the
        # data()/events()/app() publish facades (DESIGN-class-facades §3/§4); a bare
        # object.__new__ bring-up (no __init__) needs these set so _stream_sink() and
        # the EdgeCommonsInstance constructor don't AttributeError. None is a valid value for
        # both (no streaming configured; the facades default their own clock).
        gg._streams = None
        gg._clock = None

        class Cm:
            def get_component_identity(self):
                return _identity() if identity else None

            def is_topic_include_root(self):
                return include_root

            def get_instance_ids(self):
                return []
        gg._config_manager = Cm()
        return gg

    def test_uns_bound_to_component_identity_and_cached(self):
        gg = self._gg()
        uns = gg.uns()
        # D-U28: gg.uns() is component scope (no instance token).
        assert uns.topic(UnsClass.STATE) == "ecv1/gw-01/opcua-adapter/state"
        assert gg.uns() is uns  # cached

    def test_uns_include_root(self):
        gg = self._gg(include_root=True)
        assert gg.uns().topic(UnsClass.STATE) == \
            "ecv1/dallas/gw-01/opcua-adapter/state"

    def test_uns_before_init_raises(self):
        gg = object.__new__(EdgeCommons)
        gg._uns = None
        gg._instance_handles = {}
        gg._config_manager = None
        with pytest.raises(RuntimeError):
            gg.uns()

    def test_uns_without_resolved_identity_raises(self):
        gg = self._gg(identity=False)
        with pytest.raises(RuntimeError):
            gg.uns()

    def test_instance_validates_token(self):
        gg = self._gg()
        with pytest.raises(UnsValidationError) as e:
            gg.instance("in+st")
        assert e.value.code == UnsValidationError.BAD_CHAR

    def test_instance_cached_per_id(self):
        gg = self._gg()
        h1 = gg.instance("kep1")
        assert gg.instance("kep1") is h1
        assert gg.instance("kep2") is not h1

    def test_unknown_instance_id_is_allowed(self):
        # The handle is NOT verified against component.instances[] — instances may be
        # created dynamically; the token rule is the only gate (§3).
        gg = self._gg()
        assert gg.instance("dynamic-1").id() == "dynamic-1"
