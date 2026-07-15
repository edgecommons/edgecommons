"""Deterministic unit tests for the ``data()``/``events()``/``app()`` publish facades
(DESIGN-class-facades, ``docs/platform/DESIGN-class-facades.md``) and their value types
(:class:`Channel`, :class:`Quality`, :class:`Severity`,
:class:`~edgecommons.facades.signal_update.SignalUpdate`/``Sample``) -- the Python mirror
of the Java canonical's ``DataFacadeTest``/``EventsFacadeTest``/``AppFacadeTest``/
``FacadeValueTypesTest``, plus the ``EdgeCommonsInstance``/``EdgeCommons`` accessor wiring
(DESIGN-class-facades §3, D6).

Cross-language conformance (topic/body shapes pinned by ``uns-test-vectors/``) lives in
``test_facades_vectors.py``; this file covers the behavior the vectors don't reach:
constructor validation, the raw escape hatch, config-driven channel resolution (both
tiers), northbound/stream transport-failure isolation, the value-type helpers, and the
``EdgeCommonsInstance``/``EdgeCommons`` wiring.
"""
from datetime import datetime, timezone

import pytest

from edgecommons.facades.app_facade import AppFacade
from edgecommons.facades.channel import Channel
from edgecommons.facades.data_facade import DataFacade
from edgecommons.facades.events_facade import EventsFacade
from edgecommons.facades.quality import Quality
from edgecommons.facades.severity import Severity
from edgecommons.facades.signal_update import Sample, SignalUpdate, SignalUpdateBuilder
from edgecommons.facades.util import format_instant, parse_iso_to_epoch_millis
from edgecommons.edgecommons_instance import EdgeCommonsInstance
from edgecommons.messaging.identity import HierEntry, MessageIdentity

NOW = "2026-07-01T12:00:00Z"
FIXED_INSTANT = datetime(2026, 7, 1, 12, 0, 0, tzinfo=timezone.utc)


def FIXED_CLOCK():
    return FIXED_INSTANT


IDENTITY = MessageIdentity([HierEntry("device", "gw-01")], "opcua-adapter", "main")


class _FakeConfigManager:
    def __init__(self, identity=IDENTITY, instances=None, global_cfg=None):
        self._identity = identity
        self._instances = instances or {}
        self._global = global_cfg or {}

    def get_component_identity(self):
        return self._identity

    def get_instance_config(self, instance_id):
        return self._instances[instance_id]

    def get_global_config(self):
        return self._global

    def get_tag_config(self):
        return None


class _RecordingMessaging:
    def __init__(self):
        self.local = []
        self.iotcore = []
        self.iotcore_qos = []

    def publish(self, topic, msg):
        self.local.append((topic, msg))

    def publish_northbound(self, topic, msg, qos):
        self.iotcore.append((topic, msg))
        self.iotcore_qos.append(qos)


class _ThrowingIotCoreMessaging(_RecordingMessaging):
    def publish_northbound(self, topic, msg, qos):
        raise RuntimeError("iot core down")


def _uns(instance="kep1"):
    from edgecommons.uns import Uns

    return Uns(IDENTITY.with_instance(instance), False)


# ===================== Channel =====================


class TestChannel:
    def test_from_config_parses_every_recognized_form(self):
        assert Channel.from_config("local") is Channel.LOCAL
        assert Channel.from_config("LOCAL") is Channel.LOCAL
        assert Channel.from_config("northbound") is Channel.NORTHBOUND
        stream = Channel.from_config("stream:hot")
        assert stream.kind is Channel.Kind.STREAM
        assert stream.stream_name == "hot"

    def test_from_config_yields_none_for_absent_or_unrecognized(self):
        assert Channel.from_config(None) is None
        assert Channel.from_config("") is None
        assert Channel.from_config("   ") is None
        assert Channel.from_config("bogus") is None
        assert Channel.from_config("iotcore") is None
        assert Channel.from_config("iot_core") is None
        assert Channel.from_config("stream:") is None, "an empty stream name is not valid"

    def test_stream_rejects_empty_name(self):
        with pytest.raises(ValueError):
            Channel.stream("")
        with pytest.raises(ValueError):
            Channel.stream(None)

    def test_equality_hash_and_string_form(self):
        assert Channel.LOCAL == Channel.LOCAL
        assert Channel.stream("hot") == Channel.stream("hot")
        assert Channel.stream("hot") != Channel.stream("cold")
        assert Channel.LOCAL != Channel.NORTHBOUND
        assert Channel.LOCAL != "local"
        assert hash(Channel.stream("hot")) == hash(Channel.stream("hot"))
        assert str(Channel.LOCAL) == "local"
        assert str(Channel.NORTHBOUND) == "northbound"
        assert str(Channel.stream("hot")) == "stream:hot"
        assert "stream:hot" in repr(Channel.stream("hot"))


# ===================== Quality / Severity =====================


class TestQualityAndSeverity:
    def test_quality_wire_tokens(self):
        assert Quality.GOOD.wire() == "GOOD"
        assert Quality.BAD.wire() == "BAD"
        assert Quality.UNCERTAIN.wire() == "UNCERTAIN"
        assert Quality.from_wire("GOOD") is Quality.GOOD
        assert Quality.from_wire("good") is None, "wire tokens are UPPERCASE"
        assert Quality.from_wire("nope") is None

    def test_severity_wire_tokens(self):
        assert Severity.CRITICAL.wire() == "critical"
        assert Severity.from_wire("info") is Severity.INFO
        assert Severity.from_wire("INFO") is None, "wire tokens are lowercase"
        assert Severity.from_wire("nope") is None


# ===================== SignalUpdate / Sample =====================


class TestSignalUpdateAndSample:
    def test_sample_factories_set_the_expected_fields(self):
        a = Sample.of(1.0)
        assert a.value == 1.0
        assert a.quality is None

        b = Sample.of(2, Quality.BAD)
        assert b.quality is Quality.BAD
        assert b.source_ts is None

        c = Sample.of(3, Quality.UNCERTAIN, "2026-01-01T00:00:00Z")
        assert c.source_ts == "2026-01-01T00:00:00Z"

    def test_builder_accessors(self):
        address = {"ns": 2}
        update = (
            SignalUpdateBuilder("sig-1")
            .name("Signal One")
            .address(address)
            .add_sample(1.0)
            .build()
        )
        assert update.signal_id == "sig-1"
        assert update.signal_name == "Signal One"
        assert update.signal_address == address
        assert update.effective_signal_path == "sig-1", "signal_path defaults to signal_id"
        assert update.via is None
        assert len(update.samples) == 1
        assert update.device is None

        with_path = (
            SignalUpdateBuilder("sig-1")
            .signal_path("a/b")
            .via(Channel.NORTHBOUND)
            .add_samples(update.samples)
            .build()
        )
        assert with_path.effective_signal_path == "a/b"
        assert with_path.via is Channel.NORTHBOUND

    def test_add_sample_accepts_a_prebuilt_sample_or_raw_value(self):
        sample = Sample(1.0, Quality.BAD, "raw", None, None)
        update = SignalUpdateBuilder("sig-1").add_sample(sample).build()
        assert update.samples[0] is sample

    def test_device_from_parts_omits_none_fields(self):
        update = SignalUpdateBuilder("sig-1").device(adapter="opcua").add_sample(1.0).build()
        assert update.device == {"adapter": "opcua"}

    def test_detached_builder_publish_raises(self):
        detached = SignalUpdateBuilder("temp").add_sample(1.0)
        with pytest.raises(RuntimeError):
            detached.publish()


# ===================== DataFacade =====================


class TestDataFacade:
    def test_constructor_validates_required_args(self):
        messaging = _RecordingMessaging()
        config = _FakeConfigManager()
        uns = _uns()
        with pytest.raises(ValueError):
            DataFacade(None, "kep1", uns, messaging)
        with pytest.raises(ValueError):
            DataFacade(config, "kep1", None, messaging)
        with pytest.raises(ValueError):
            DataFacade(config, "kep1", uns, None)
        # D-U28: an empty/None instance_id is component scope (accepted, no raise).
        assert DataFacade(config, None, uns, messaging).instance_id() is None

    def test_instance_id_accessor(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        assert facade.instance_id() == "kep1"

    def test_value_shorthand_publishes_one_sample(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 21.5)
        assert len(messaging.local) == 1
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/kep1/data/temp"
        sample = msg.to_dict()["body"]["samples"][0]
        assert sample["quality"] == "GOOD"
        assert sample["qualityRaw"] == "unspecified"
        assert sample["serverTs"] == NOW

    def test_value_shorthand_with_explicit_quality(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 0, Quality.BAD)
        sample = messaging.local[0][1].to_dict()["body"]["samples"][0]
        assert sample["quality"] == "BAD"
        assert "qualityRaw" not in sample

    def test_publish_body_raw_escape_hatch(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        raw = {"anything": "goes", "n": 7}
        facade.publish_body("custom", raw)
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/kep1/data/custom"
        assert msg.to_dict()["body"] == raw, "the escape hatch applies no defaulting"

    def test_publish_body_rejects_none_body(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        with pytest.raises(ValueError):
            facade.publish_body("custom", None)

    def test_publish_body_rejects_empty_signal_path(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        with pytest.raises(ValueError):
            facade.publish_body("", {"a": 1})

    def test_missing_signal_id_is_rejected(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        update = SignalUpdateBuilder(None).add_sample(1.0).build()
        with pytest.raises(ValueError):
            facade.publish_update(update)

    def test_empty_samples_is_rejected(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        update = SignalUpdateBuilder("temp").build()
        with pytest.raises(ValueError):
            facade.publish_update(update)

    def test_quality_only_sample_with_no_value_is_rejected(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        update = SignalUpdateBuilder("temp").add_sample(
            Sample(None, Quality.BAD, None, None, None)
        ).build()
        with pytest.raises(ValueError):
            facade.publish_update(update)

    def test_explicit_quality_raw_passed_through_verbatim(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.signal("temp").add_sample(
            Sample(21.5, Quality.GOOD, "Good", "2026-07-01T11:00:00Z", "2026-07-01T11:00:01Z")
        ).publish()
        sample = messaging.local[0][1].to_dict()["body"]["samples"][0]
        assert sample["qualityRaw"] == "Good"
        assert sample["sourceTs"] == "2026-07-01T11:00:00Z"
        assert sample["serverTs"] == "2026-07-01T11:00:01Z", "caller serverTs is not overwritten"

    def test_northbound_override_routes_to_iot_core(self):
        from edgecommons.messaging.qos import Qos

        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.signal("temp").add_sample(21.5).via(Channel.NORTHBOUND).publish()
        assert not messaging.local
        assert messaging.iotcore_qos == [Qos.AT_LEAST_ONCE]

    def test_northbound_transport_failure_is_swallowed(self):
        messaging = _ThrowingIotCoreMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.signal("temp").add_sample(1.0).via(Channel.NORTHBOUND).publish()  # must not raise

    def test_stream_override_appends_and_falls_back_when_no_sink(self):
        calls = []

        def sink(stream_name, partition_key, ts_millis, payload):
            calls.append((stream_name, partition_key, ts_millis, payload))

        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, sink, FIXED_CLOCK)
        facade.signal("ns=2;s=Line1.Temp").add_sample(21.5).via(Channel.stream("hot")).publish()
        assert not messaging.local
        assert len(calls) == 1
        stream_name, partition_key, ts_millis, payload = calls[0]
        assert stream_name == "hot"
        assert partition_key == "ns=2;s=Line1.Temp"
        assert ts_millis == int(FIXED_INSTANT.timestamp() * 1000)

        # No sink configured -> falls back to LOCAL (readiness/no-streaming -> local).
        facade_no_sink = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, None, FIXED_CLOCK)
        facade_no_sink.signal("temp").add_sample(1.0).via(Channel.stream("hot")).publish()
        assert len(messaging.local) == 1

    def test_stream_append_failure_is_swallowed(self):
        def throwing_sink(stream_name, partition_key, ts_millis, payload):
            raise RuntimeError("stream buffer full")

        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, throwing_sink, FIXED_CLOCK)
        facade.signal("temp").add_sample(1.0).via(Channel.stream("hot")).publish()
        assert not messaging.local, "it tried the stream, not the bus"

    def test_configured_instance_publish_channel_routes_without_override(self):
        config = _FakeConfigManager(instances={"kep1": {"publish": {"channel": "northbound"}}})
        messaging = _RecordingMessaging()
        facade = DataFacade(config, "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 21.5)
        assert messaging.iotcore, "config publish.channel=northbound routes without an override"

    def test_configured_global_publish_channel_is_the_fallback(self):
        config = _FakeConfigManager(global_cfg={"publish": {"channel": "northbound"}})
        messaging = _RecordingMessaging()
        facade = DataFacade(config, "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 21.5)
        assert messaging.iotcore

    def test_per_call_override_wins_over_config_default(self):
        config = _FakeConfigManager(instances={"kep1": {"publish": {"channel": "northbound"}}})
        messaging = _RecordingMessaging()
        facade = DataFacade(config, "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.signal("temp").add_sample(21.5).via(Channel.LOCAL).publish()
        assert messaging.local and not messaging.iotcore

    def test_unrecognized_config_channel_falls_through_to_local(self):
        config = _FakeConfigManager(instances={"kep1": {"publish": {"channel": "bogus"}}})
        messaging = _RecordingMessaging()
        facade = DataFacade(config, "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 1.0)
        assert messaging.local and not messaging.iotcore

    def test_config_lookup_exception_falls_through_to_local(self):
        class ExplodingConfig(_FakeConfigManager):
            def get_instance_config(self, instance_id):
                raise KeyError("no such instance")

            def get_global_config(self):
                raise RuntimeError("boom")

        messaging = _RecordingMessaging()
        facade = DataFacade(ExplodingConfig(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 1.0)
        assert messaging.local

    def test_resolve_channel_precedence(self):
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), _RecordingMessaging())
        assert facade.resolve_channel(Channel.NORTHBOUND) is Channel.NORTHBOUND
        assert facade.resolve_channel(None) is Channel.LOCAL

    def test_channel_path_is_sanitized(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("a+b", 1.0)
        assert messaging.local[0][0] == "ecv1/gw-01/opcua-adapter/kep1/data/a_b"

    def test_multi_token_signal_path_becomes_multiple_channel_tokens(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("a/b", 1.0)
        assert messaging.local[0][0] == "ecv1/gw-01/opcua-adapter/kep1/data/a/b"

    def test_fluent_builder_constructs_the_full_southbound_body(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        address = {"ns": 2, "nodeId": "Line1.Temp"}
        facade.signal("ns=2;s=Line1.Temp").name("Line 1 Temperature").address(address).device(
            "opcua", "kep1", "opc.tcp://host:4840"
        ).add_sample(21.5).signal_path("press12/temperature").publish()

        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/kep1/data/press12/temperature"
        body = msg.to_dict()["body"]
        assert body["device"]["adapter"] == "opcua"
        assert body["signal"]["id"] == "ns=2;s=Line1.Temp"
        assert body["signal"]["name"] == "Line 1 Temperature"
        assert body["signal"]["address"]["ns"] == 2

    def test_batch_samples_are_published_in_order(self):
        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.signal("flow").add_sample(1.0).add_sample(2.0, Quality.UNCERTAIN).publish()
        assert len(messaging.local[0][1].to_dict()["body"]["samples"]) == 2


# ===================== EventsFacade =====================


class TestEventsFacade:
    def _facade(self, messaging=None):
        messaging = messaging if messaging is not None else _RecordingMessaging()
        config = _FakeConfigManager()
        from edgecommons.uns import Uns

        uns = Uns(IDENTITY, False)
        return EventsFacade(config, "main", uns, messaging, FIXED_CLOCK), messaging

    def test_constructor_validates_required_args(self):
        with pytest.raises(ValueError):
            EventsFacade(None, "main", _uns(), _RecordingMessaging())
        with pytest.raises(ValueError):
            EventsFacade(_FakeConfigManager(), "main", None, _RecordingMessaging())
        with pytest.raises(ValueError):
            EventsFacade(_FakeConfigManager(), "main", _uns(), None)
        # D-U28: an empty/None instance_id is component scope (accepted, no raise).
        EventsFacade(_FakeConfigManager(), None, _uns(), _RecordingMessaging())

    def test_emit_derives_channel_and_defaults_timestamp(self):
        facade, messaging = self._facade()
        facade.emit("write-rejected", "not in allow-list", severity=Severity.WARNING)
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/evt/warning/write-rejected"
        body = msg.to_dict()["body"]
        assert body["severity"] == "warning"
        assert body["type"] == "write-rejected"
        assert body["message"] == "not in allow-list"
        assert body["timestamp"] == NOW
        assert "context" not in body
        assert "alarm" not in body

    def test_message_only_emit_defaults_severity_to_info(self):
        facade, messaging = self._facade()
        facade.emit("door-open", "front door opened")
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/evt/info/door-open"
        assert msg.to_dict()["body"]["severity"] == "info"

    def test_context_is_included_when_provided(self):
        facade, messaging = self._facade()
        ctx = {"celsius": 95.0}
        facade.emit("overtemp", "too hot", ctx, Severity.CRITICAL)
        assert messaging.local[0][1].to_dict()["body"]["context"] == ctx

    def test_type_is_sanitized_for_channel_but_rides_body_verbatim(self):
        facade, messaging = self._facade()
        facade.emit("a+b", None, None, Severity.INFO)
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/evt/info/a_b"
        assert msg.to_dict()["body"]["type"] == "a+b"

    def test_raise_alarm_defaults_to_critical_with_alarm_active_true(self):
        facade, messaging = self._facade()
        facade.raise_alarm("connection-lost", "link down", {"connected": False})
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost"
        body = msg.to_dict()["body"]
        assert body["severity"] == "critical"
        assert body["alarm"] is True
        assert body["active"] is True

    def test_clear_alarm_shares_the_raise_channel_with_active_false(self):
        facade, messaging = self._facade()
        facade.clear_alarm("connection-lost")
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/evt/critical/connection-lost"
        body = msg.to_dict()["body"]
        assert body["alarm"] is True
        assert body["active"] is False
        assert "message" not in body

    def test_alarm_severity_is_overridable(self):
        facade, messaging = self._facade()
        facade.raise_alarm("degraded", "running degraded", severity=Severity.WARNING)
        assert messaging.local[0][0] == "ecv1/gw-01/opcua-adapter/main/evt/warning/degraded"

    def test_via_northbound_routes_to_iot_core(self):
        from edgecommons.messaging.qos import Qos

        facade, messaging = self._facade()
        facade.via(Channel.NORTHBOUND).emit("overtemp", "escalate", severity=Severity.CRITICAL)
        assert messaging.iotcore_qos == [Qos.AT_LEAST_ONCE]

    def test_via_stream_is_rejected(self):
        facade, _ = self._facade()
        with pytest.raises(ValueError):
            facade.via(Channel.stream("hot"))

    def test_empty_type_is_rejected(self):
        facade, messaging = self._facade()
        with pytest.raises(ValueError):
            facade.emit("", "msg")
        assert not messaging.local

    def test_northbound_transport_failure_is_swallowed(self):
        facade, _ = self._facade(_ThrowingIotCoreMessaging())
        facade.via(Channel.NORTHBOUND).emit("overtemp", "x")  # must not raise


# ===================== AppFacade =====================


class TestAppFacade:
    def _facade(self, messaging=None):
        messaging = messaging if messaging is not None else _RecordingMessaging()
        from edgecommons.uns import Uns

        uns = Uns(IDENTITY, False)
        return AppFacade(_FakeConfigManager(), "main", uns, messaging), messaging

    def test_constructor_validates_required_args(self):
        with pytest.raises(ValueError):
            AppFacade(None, "main", _uns(), _RecordingMessaging())
        with pytest.raises(ValueError):
            AppFacade(_FakeConfigManager(), "main", None, _RecordingMessaging())
        with pytest.raises(ValueError):
            AppFacade(_FakeConfigManager(), "main", _uns(), None)
        # D-U28: an empty/None instance_id is component scope (accepted, no raise).
        AppFacade(_FakeConfigManager(), None, _uns(), _RecordingMessaging())

    def test_publishes_verbatim_body_with_named_header_onto_app_channel(self):
        facade, messaging = self._facade()
        body = {"orderId": "A-42", "qty": 3}
        facade.publish("OrderReceived", "order/received", body)
        topic, msg = messaging.local[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/app/order/received"
        assert msg.to_dict()["header"]["name"] == "OrderReceived"
        assert msg.to_dict()["body"] == body

    def test_channel_is_sanitized(self):
        facade, messaging = self._facade()
        facade.publish("Ping", "a+b", {})
        assert messaging.local[0][0] == "ecv1/gw-01/opcua-adapter/main/app/a_b"

    def test_northbound_routing_goes_to_iot_core(self):
        from edgecommons.messaging.qos import Qos

        facade, messaging = self._facade()
        facade.publish("CloudEvent", "cloud", {}, Channel.NORTHBOUND)
        assert messaging.iotcore_qos == [Qos.AT_LEAST_ONCE]

    def test_stream_routing_is_rejected(self):
        facade, _ = self._facade()
        with pytest.raises(ValueError):
            facade.publish("X", "c", {}, Channel.stream("hot"))

    def test_empty_name_or_channel_is_rejected(self):
        facade, messaging = self._facade()
        with pytest.raises(ValueError):
            facade.publish("", "c", {})
        with pytest.raises(ValueError):
            facade.publish("X", "", {})
        assert not messaging.local

    def test_northbound_transport_failure_is_swallowed(self):
        facade, _ = self._facade(_ThrowingIotCoreMessaging())
        facade.publish("X", "c", {}, Channel.NORTHBOUND)  # must not raise


# ===================== EdgeCommonsInstance wiring =====================


class TestEdgeCommonsInstanceFacades:
    def _cm(self):
        class Cm:
            def get_component_identity(self):
                return IDENTITY

            def get_instance_config(self, instance_id):
                raise KeyError(instance_id)

            def get_global_config(self):
                return {}

            def get_tag_config(self):
                return None

        return Cm()

    def test_data_events_app_are_lazily_cached(self):
        handle = EdgeCommonsInstance("kep1", self._cm(), False, messaging_client=_RecordingMessaging())
        assert handle.data() is handle.data()
        assert handle.events() is handle.events()
        assert handle.app() is handle.app()

    def test_facades_require_a_messaging_client(self):
        handle = EdgeCommonsInstance("kep1", self._cm(), False)
        with pytest.raises(RuntimeError):
            handle.data()
        with pytest.raises(RuntimeError):
            handle.events()
        with pytest.raises(RuntimeError):
            handle.app()

    def test_data_facade_publishes_through_the_bound_instance(self):
        messaging = _RecordingMessaging()
        handle = EdgeCommonsInstance("kep1", self._cm(), False, messaging_client=messaging, clock=FIXED_CLOCK)
        handle.data().publish("temp", 1.0)
        assert messaging.local[0][0] == "ecv1/gw-01/opcua-adapter/kep1/data/temp"


# ===================== EdgeCommons convenience accessors =====================


class TestEdgeCommonsFacadeAccessors:
    def _gg(self):
        from edgecommons.edgecommons import EdgeCommons

        gg = object.__new__(EdgeCommons)
        gg._uns = None
        gg._instance_handles = {}
        gg._component_handle = None
        gg._streams = None
        gg._clock = FIXED_CLOCK
        gg._config_manager = self._Cm()
        return gg

    class _Cm:
        def get_component_identity(self):
            return IDENTITY

        def is_topic_include_root(self):
            return False

        def get_instance_ids(self):
            return []

        def get_instance_config(self, instance_id):
            raise KeyError(instance_id)

        def get_global_config(self):
            return {}

    def test_data_events_app_are_cached_component_scope_facades(self):
        # D-U28: gg.data()/events()/app() are the component-scope facades (no instance
        # token), cached and distinct from an instance-scoped handle's facades.
        gg = self._gg()
        assert gg.data() is gg.data()  # cached component-scope handle
        assert gg.events() is gg.events()
        assert gg.app() is gg.app()
        assert gg.data() is not gg.instance("main").data()
        assert gg.data().instance_id() is None  # component scope (DataFacade exposes the accessor)


# ===================== util helpers (format_instant / parse_iso_to_epoch_millis) =====================


class TestFacadeUtilHelpers:
    def test_format_instant_whole_second(self):
        assert format_instant(FIXED_INSTANT) == NOW

    def test_format_instant_millisecond_precision(self):
        dt = datetime(2026, 7, 1, 12, 0, 0, 500_000, tzinfo=timezone.utc)
        assert format_instant(dt) == "2026-07-01T12:00:00.500Z"

    def test_format_instant_microsecond_precision(self):
        dt = datetime(2026, 7, 1, 12, 0, 0, 123_456, tzinfo=timezone.utc)
        assert format_instant(dt) == "2026-07-01T12:00:00.123456Z"

    def test_parse_iso_to_epoch_millis_whole_second(self):
        assert parse_iso_to_epoch_millis(NOW) == int(FIXED_INSTANT.timestamp() * 1000)

    def test_parse_iso_to_epoch_millis_single_fractional_digit(self):
        # "2026-07-01T11:59:59.5Z" from the data.json full-sample-passthrough vector.
        millis = parse_iso_to_epoch_millis("2026-07-01T11:59:59.5Z")
        expected = datetime(2026, 7, 1, 11, 59, 59, 500_000, tzinfo=timezone.utc)
        assert millis == int(expected.timestamp() * 1000)

    def test_parse_iso_to_epoch_millis_falls_back_on_unparseable_input(self):
        before = int(datetime.now(timezone.utc).timestamp() * 1000)
        millis = parse_iso_to_epoch_millis("not-a-timestamp")
        after = int(datetime.now(timezone.utc).timestamp() * 1000)
        assert before <= millis <= after


# ===================== remaining edge branches (config-channel type + malformed body) ====


class TestRemainingEdgeBranches:
    def test_publish_channel_non_string_value_is_ignored(self):
        config = _FakeConfigManager(instances={"kep1": {"publish": {"channel": 123}}})
        messaging = _RecordingMessaging()
        facade = DataFacade(config, "kep1", _uns(), messaging, clock=FIXED_CLOCK)
        facade.publish("temp", 1.0)
        assert messaging.local and not messaging.iotcore, (
            "a non-string publish.channel value must not resolve to a channel"
        )

    def test_first_server_ts_millis_falls_back_when_body_is_malformed(self):
        """A malformed raw body (via the escape hatch) whose 'samples[0]' is not
        dict-shaped must not raise -- it falls back to 'now' for the stream timestamp."""
        calls = []

        def sink(stream_name, partition_key, ts_millis, payload):
            calls.append((stream_name, partition_key, ts_millis, payload))

        messaging = _RecordingMessaging()
        facade = DataFacade(_FakeConfigManager(), "kep1", _uns(), messaging, sink, FIXED_CLOCK)
        before = int(datetime.now(timezone.utc).timestamp() * 1000)
        facade.publish_body("x", {"samples": [5]}, via=Channel.stream("hot"))
        after = int(datetime.now(timezone.utc).timestamp() * 1000)
        assert len(calls) == 1
        assert before <= calls[0][2] <= after

    def test_channel_for_rejects_empty_type_directly(self):
        from edgecommons.uns import Uns

        facade = EventsFacade(_FakeConfigManager(), "main", Uns(IDENTITY, False),
                              _RecordingMessaging(), FIXED_CLOCK)
        with pytest.raises(ValueError):
            facade.channel_for(Severity.INFO, "")
