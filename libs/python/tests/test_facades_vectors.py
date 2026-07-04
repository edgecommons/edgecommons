"""Cross-language class-facade conformance: replay ``uns-test-vectors/{data,evt,app}.json``
against the live Python ``data()``/``events()``/``app()`` facades (DESIGN-class-facades,
`docs/platform/DESIGN-class-facades.md`), plus the refreshed ``envelopes.json`` data/evt/app
goldens (generically covered by ``test_uns_vectors.py::test_vector_envelope``, exercised
again here through the facades themselves for the topic).

Mirrors the ``test_uns_vectors.py`` loader pattern (skip if the shared vectors are not
checked out). All cases use a fixed injected clock (``2026-07-01T12:00:00Z``) so
``serverTs``/``timestamp`` defaults are deterministic, at parity with the Java
``DataFacadeTest``/``EventsFacadeTest``/``AppFacadeTest`` fixed-clock discipline.

- **data.json** — replayed through ``DataFacade.signal(...).add_sample(...).publish()``;
  pins the ``GOOD`` quality default (+ ``qualityRaw: "unspecified"``), ``serverTs=now``,
  the samples wrapper, channel sanitization, the missing-``signal.id``/no-samples/
  value-less-sample reject cases, and LOCAL/northbound/stream channel routing.
- **evt.json** — replayed through ``EventsFacade.emit``/``raise_alarm``/``clear_alarm``;
  pins the ``evt/{severity}/{type}`` channel derived from the body, the four severity
  tokens, ``timestamp=now``, and the alarm ``alarm``/``active`` fields.
- **app.json** — replayed through ``AppFacade.publish``; pins the verbatim body, the
  header ``name``, and the ``app/{channel}`` topic (sanitized).
"""
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Optional

import pytest

from ggcommons.facades.app_facade import AppFacade
from ggcommons.facades.channel import Channel
from ggcommons.facades.data_facade import DataFacade
from ggcommons.facades.events_facade import EventsFacade
from ggcommons.facades.quality import Quality
from ggcommons.facades.severity import Severity
from ggcommons.facades.signal_update import Sample
from ggcommons.messaging.identity import HierEntry, MessageIdentity
from ggcommons.uns import Uns

VECTORS_DIR = Path(__file__).resolve().parents[3] / "uns-test-vectors"

pytestmark = pytest.mark.skipif(
    not (VECTORS_DIR / "data.json").exists(), reason="uns-test-vectors not present"
)

# The vectors are generated against this fixed identity + clock (Java DataFacadeTest/
# EventsFacadeTest/AppFacadeTest use the same "gw-01"/"opcua-adapter"/2026-07-01T12:00:00Z
# fixture).
NOW = "2026-07-01T12:00:00Z"
FIXED_INSTANT = datetime(2026, 7, 1, 12, 0, 0, tzinfo=timezone.utc)


def FIXED_CLOCK():
    return FIXED_INSTANT


IDENTITY = MessageIdentity([HierEntry("device", "gw-01")], "opcua-adapter", "main")


def _load(name: str):
    return json.loads((VECTORS_DIR / name).read_text(encoding="utf-8"))


def _cases(file_name: str) -> List[Dict[str, Any]]:
    if not (VECTORS_DIR / file_name).exists():
        return []
    return _load(file_name)["cases"]


def _case_ids(cases):
    return [c["name"] for c in cases]


class _FakeConfigManager:
    """A minimal config-manager double exposing exactly what the facades read: the
    resolved identity and the (absent, by default) ``publish.channel`` config tiers."""

    def __init__(self, identity: MessageIdentity, instances: Optional[dict] = None,
                global_cfg: Optional[dict] = None):
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
    """A messaging double recording every LOCAL/northbound publish (no live broker)."""

    def __init__(self):
        self.local: List[tuple] = []
        self.iotcore: List[tuple] = []

    def publish(self, topic, msg):
        self.local.append((topic, msg))

    def publish_to_iot_core(self, topic, msg, qos):
        self.iotcore.append((topic, msg))


class _RecordingStreamSink:
    """A stream-sink double recording every append (no native streaming engine)."""

    def __init__(self):
        self.calls: List[tuple] = []

    def __call__(self, stream_name, partition_key, timestamp_ms, payload):
        self.calls.append((stream_name, partition_key, timestamp_ms, payload))


def _channel_override(name: Optional[str]) -> Optional[Channel]:
    """Parses the vectors' ``input.override`` field (``"northbound"`` / ``"stream:<name>"``)
    into a :class:`Channel`."""
    if name is None:
        return None
    if name == "northbound":
        return Channel.NORTHBOUND
    if name.startswith("stream:"):
        return Channel.stream(name[len("stream:"):])
    raise AssertionError(f"unrecognized vector override '{name}'")


# ===================== data.json =====================


@pytest.mark.parametrize("case", _cases("data.json"), ids=_case_ids(_cases("data.json")))
def test_data_vector(case):
    inp = case["input"]
    expected = case["expected"]

    messaging = _RecordingMessaging()
    stream_sink = _RecordingStreamSink()
    config = _FakeConfigManager(IDENTITY)
    uns = Uns(IDENTITY.with_instance("kep1"), False)
    facade = DataFacade(config, "kep1", uns, messaging, stream_sink, FIXED_CLOCK)

    builder = facade.signal(inp.get("signalId"))
    if "signalName" in inp:
        builder.name(inp["signalName"])
    if "signalAddress" in inp:
        builder.address(inp["signalAddress"])
    if "device" in inp:
        builder.device(block=inp["device"])
    for raw_sample in inp["samples"]:
        quality = Quality.from_wire(raw_sample["quality"]) if "quality" in raw_sample else None
        sample = Sample(
            raw_sample.get("value"), quality, raw_sample.get("qualityRaw"),
            raw_sample.get("sourceTs"), raw_sample.get("serverTs"),
        )
        builder.add_sample(sample)
    if "signalPath" in inp:
        builder.signal_path(inp["signalPath"])
    override = _channel_override(inp.get("override"))
    if override is not None:
        builder.via(override)

    if expected.get("throws"):
        with pytest.raises(ValueError):
            builder.publish()
        assert not messaging.local and not messaging.iotcore and not stream_sink.calls, (
            "'" + case["name"] + "' - nothing should reach the wire on a reject"
        )
        return

    builder.publish()

    route = expected["route"]
    if route == "local":
        assert len(messaging.local) == 1, "'" + case["name"] + "' local publish count"
        topic, msg = messaging.local[0]
        assert topic == expected["topic"], "'" + case["name"] + "' topic"
        assert msg.to_dict()["body"] == expected["body"], "'" + case["name"] + "' body"
    elif route == "northbound":
        assert len(messaging.iotcore) == 1, "'" + case["name"] + "' iotcore publish count"
        topic, msg = messaging.iotcore[0]
        assert topic == expected["topic"], "'" + case["name"] + "' topic"
        assert msg.to_dict()["body"] == expected["body"], "'" + case["name"] + "' body"
    else:
        assert route.startswith("stream:"), "'" + case["name"] + "' unrecognized route"
        assert len(stream_sink.calls) == 1, "'" + case["name"] + "' stream append count"
        assert not messaging.local, "'" + case["name"] + "' - the stream route must not fall back to LOCAL"
        stream_name, partition_key, timestamp_ms, payload = stream_sink.calls[0]
        assert f"stream:{stream_name}" == route, "'" + case["name"] + "' stream name"
        assert partition_key == expected["partitionKey"], "'" + case["name"] + "' partition key"
        env = json.loads(payload.decode("utf-8"))
        assert env["body"] == expected["body"], "'" + case["name"] + "' streamed body"


def test_data_stream_falls_back_to_local_when_no_sink_configured():
    """Readiness/no-streaming -> local (D1a): a ``stream:`` route with no stream sink
    wired falls back to a LOCAL publish rather than dropping the record or raising."""
    messaging = _RecordingMessaging()
    config = _FakeConfigManager(IDENTITY)
    uns = Uns(IDENTITY.with_instance("kep1"), False)
    facade = DataFacade(config, "kep1", uns, messaging, None, FIXED_CLOCK)

    facade.signal("temp").add_sample(21.5).via(Channel.stream("hot")).publish()

    assert len(messaging.local) == 1
    assert not messaging.iotcore


# ===================== evt.json =====================


@pytest.mark.parametrize("case", _cases("evt.json"), ids=_case_ids(_cases("evt.json")))
def test_evt_vector(case):
    inp = case["input"]
    expected = case["expected"]

    messaging = _RecordingMessaging()
    config = _FakeConfigManager(IDENTITY)
    uns = Uns(IDENTITY.with_instance("main"), False)
    facade = EventsFacade(config, "main", uns, messaging, FIXED_CLOCK)

    override = _channel_override(inp.get("override"))
    target = facade.via(override) if override is not None else facade
    severity = Severity.from_wire(inp["severity"]) if "severity" in inp else None

    kind = inp["kind"]
    if kind == "emit":
        target.emit(inp["type"], inp.get("message"), inp.get("context"), severity)
    elif kind == "raise":
        target.raise_alarm(inp["type"], inp.get("message"), inp.get("context"), severity)
    elif kind == "clear":
        target.clear_alarm(inp["type"], inp.get("context"), severity)
    else:
        pytest.fail(f"unrecognized evt vector kind '{kind}'")

    route = expected["route"]
    if route == "local":
        assert len(messaging.local) == 1, "'" + case["name"] + "' local publish count"
        topic, msg = messaging.local[0]
    else:
        assert route == "northbound"
        assert len(messaging.iotcore) == 1, "'" + case["name"] + "' iotcore publish count"
        topic, msg = messaging.iotcore[0]
    assert topic == expected["topic"], "'" + case["name"] + "' topic"
    assert msg.to_dict()["body"] == expected["body"], "'" + case["name"] + "' body"


def test_evt_via_rejects_stream_channel():
    config = _FakeConfigManager(IDENTITY)
    uns = Uns(IDENTITY.with_instance("main"), False)
    facade = EventsFacade(config, "main", uns, _RecordingMessaging(), FIXED_CLOCK)
    with pytest.raises(ValueError):
        facade.via(Channel.stream("hot"))


# ===================== app.json =====================


@pytest.mark.parametrize("case", _cases("app.json"), ids=_case_ids(_cases("app.json")))
def test_app_vector(case):
    inp = case["input"]
    expected = case["expected"]

    messaging = _RecordingMessaging()
    config = _FakeConfigManager(IDENTITY)
    uns = Uns(IDENTITY.with_instance("main"), False)
    facade = AppFacade(config, "main", uns, messaging)

    override = _channel_override(inp.get("override"))
    facade.publish(inp["name"], inp["channel"], inp["body"], override)

    route = expected["route"]
    if route == "local":
        assert len(messaging.local) == 1, "'" + case["name"] + "' local publish count"
        topic, msg = messaging.local[0]
    else:
        assert route == "northbound"
        assert len(messaging.iotcore) == 1, "'" + case["name"] + "' iotcore publish count"
        topic, msg = messaging.iotcore[0]
    assert topic == expected["topic"], "'" + case["name"] + "' topic"
    assert msg.to_dict()["body"] == expected["body"], "'" + case["name"] + "' body"
    assert msg.to_dict()["header"]["name"] == inp["name"], "'" + case["name"] + "' header name"


def test_app_publish_rejects_stream_channel():
    config = _FakeConfigManager(IDENTITY)
    uns = Uns(IDENTITY.with_instance("main"), False)
    facade = AppFacade(config, "main", uns, _RecordingMessaging())
    with pytest.raises(ValueError):
        facade.publish("Name", "chan", {}, Channel.stream("hot"))


# ===================== envelopes.json (data/evt/app entries) =====================
# The generic golden-envelope round-trip (header/identity/body structural equality) is
# already pinned by test_uns_vectors.py::test_vector_envelope for every class, including
# data/evt/app. Here we additionally confirm the topic each envelope's class/channel
# implies is reproducible through a facade-bound Uns instance (not just the raw builder),
# i.e. the facade and the generic envelope goldens agree on topic construction.


def _envelope_cases_for(cls_token: str):
    if not (VECTORS_DIR / "envelopes.json").exists():
        return []
    return [c for c in _load("envelopes.json")["envelopes"] if c["class"] == cls_token]


@pytest.mark.parametrize(
    "case",
    _envelope_cases_for("data") + _envelope_cases_for("evt") + _envelope_cases_for("app"),
    ids=lambda c: c["name"],
)
def test_envelope_topic_matches_facade_bound_uns(case):
    identity = MessageIdentity.from_dict(case["envelope"]["identity"])
    uns = Uns(identity, False)
    from ggcommons.uns import UnsClass

    cls = UnsClass.from_token(case["class"])
    assert uns.topic(cls, case.get("channel")) == case["topic"], (
        "'" + case["name"] + "' - facade-bound Uns must mint the same topic the golden"
        " envelope pins"
    )
