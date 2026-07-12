"""Deterministic unit tests for CommandInbox (DESIGN-uns §7.3/§9.5, the minimal
``commands()`` facade - edge-console slice S2) over fake messaging/config seams, plus
a conformance section replaying ``uns-test-vectors/commands.json`` through a live
inbox. Mirrors ``libs/java/.../commands/CommandInboxTest.java``.

Covers:
- ``start()`` subscribes exactly the own-inbox wildcard
  (``ecv1/{device}/{component}/main/cmd/#``) on the primary connection;
- each built-in verb dispatches and replies with the pinned body shape - ``ping``
  (status + uptime), ``status`` (ping's body plus the per-instance ``instances[]``
  sample - omitted when the component reports none), ``reload-config`` (ack /
  ``RELOAD_FAILED``), ``get-configuration`` (redacted config / ``NO_CONFIG``),
  ``describe`` (the discovery manifest);
- replies go to the request's ``reply_to`` with the request's ``correlation_id`` and
  the responder's identity;
- custom verbs register/dispatch (namespaced verbs included), cannot shadow
  built-ins or each other, and unregister; coded (``CommandException``) vs uncoded
  (``HANDLER_ERROR``) failures;
- unknown verbs get an ``UNKNOWN_VERB`` error reply (requests) or are ignored
  (fire-and-forget); no-``reply_to`` commands run the handler without a reply;
- malformed payloads (name mismatch, headerless, None) and the delegated
  ``set-config`` verb are ignored - never replied to, never a crash;
- ``close()`` unsubscribes the inbox and stops dispatch; lifecycle is idempotent; a
  missing resolved identity disables the inbox;
- the ``commands.json`` conformance vectors: the inbox filter, the built-in
  verb request/reply goldens replayed through a live inbox, the ``UNKNOWN_VERB``
  case, and the behavior/builtInVerbs/errorCodes constants.
"""
import hashlib
import json
from pathlib import Path

import pytest

from edgecommons.command_inbox import CommandException, CommandInbox
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.uns import UnsValidationError

INBOX_FILTER = "ecv1/test-thing/TestComponent/main/cmd/#"
REPLY_TO = "edgecommons/reply-test-1"


class FakeConfig:
    """Minimal ConfigManager stand-in: identity resolution + reply stamping."""

    def __init__(self, identity=True, include_root=False):
        self._identity = (
            MessageIdentity([HierEntry("device", "test-thing")], "TestComponent")
            if identity
            else None
        )
        self._include_root = include_root

    def get_component_identity(self):
        return self._identity

    def set_component_identity(self, identity):
        self._identity = identity

    def is_topic_include_root(self):
        return self._include_root

    def get_tag_config(self):
        return None


class _PublishedReply:
    __slots__ = ("topic", "message")

    def __init__(self, topic, message):
        self.topic = topic
        self.message = message


class FakeMessaging:
    """Records subscribe/unsubscribe/reply calls. ``simulate_message`` delivers to
    every subscription whose filter matches the given (concrete) topic via the real
    MQTT wildcard matcher, so the inbox's single ``.../cmd/#`` subscription receives
    concrete verb topics exactly like a real broker would."""

    def __init__(self):
        self._callbacks = {}
        self.published = []

    def subscribe(self, topic, callback, max_concurrency=None, max_messages=None):
        self._callbacks[topic] = callback

    def unsubscribe(self, topic):
        self._callbacks.pop(topic, None)

    def subscribed_topics(self):
        return set(self._callbacks.keys())

    def simulate_message(self, topic, message):
        for sub, callback in list(self._callbacks.items()):
            if sub == topic or MessagingClient.topic_matches_sub(sub, topic):
                callback(topic, message)

    def reply(self, request, reply):
        reply_to = None
        if request is not None and request.get_header() is not None:
            reply_to = request.get_header().reply_to
            reply.set_correlation_id(request.get_header().correlation_id)
        self.published.append(_PublishedReply(reply_to, reply))


def _topic(verb):
    return f"ecv1/test-thing/TestComponent/main/cmd/{verb}"


def _request(verb, reply_to=REPLY_TO):
    """A well-formed request for a verb: header.name = verb, pinned reply_to."""
    message = MessageBuilder.create(verb, "1.0").with_payload({}).build()
    message.make_request(reply_to)
    return message


def _notification(verb):
    """A well-formed fire-and-forget command (no reply_to)."""
    return MessageBuilder.create(verb, "1.0").with_payload({}).build()


class Harness:
    def __init__(self, uptime=42, reload_ok=True, redacted=None, instance_connectivity=None):
        self.config = FakeConfig()
        self.messaging = FakeMessaging()
        self.uptime = uptime
        self.reload_ok = reload_ok
        self.redacted = (
            {"component": {"global": {"v": 1}}} if redacted is None else redacted
        )
        self.inbox = CommandInbox(
            self.config,
            self.messaging,
            lambda: self.uptime,
            lambda: self.reload_ok,
            lambda: self.redacted,
            instance_connectivity,
        )

    def only_reply(self):
        assert len(self.messaging.published) == 1, "exactly one reply expected"
        published = self.messaging.published[0]
        assert published.topic == REPLY_TO, "the reply must go to the request's reply_to"
        return published

    def only_reply_body(self):
        return self.only_reply().message.get_body()


@pytest.fixture
def h():
    return Harness()


# ===================== subscription lifecycle =====================


def test_start_subscribes_the_own_inbox_wildcard(h):
    h.inbox.start()
    assert h.messaging.subscribed_topics() == {INBOX_FILTER}, (
        "start() must subscribe exactly the own-inbox cmd wildcard"
    )


def test_start_is_idempotent(h):
    h.inbox.start()
    h.inbox.start()
    assert h.messaging.subscribed_topics() == {INBOX_FILTER}


def test_missing_identity_disables_the_inbox(h):
    h.config.set_component_identity(None)  # the mock/test bring-up case
    h.inbox.start()
    assert not h.messaging.subscribed_topics(), (
        "no resolved identity -> no inbox subscription (WARN + disabled)"
    )
    h.inbox.close()  # must not raise


def test_close_unsubscribes_and_stops_dispatch(h):
    h.inbox.start()
    h.inbox.close()
    assert not h.messaging.subscribed_topics(), (
        "close() must unsubscribe the inbox (unsubscribe-before-exit)"
    )
    # A late (queued) delivery after close is ignored.
    h.messaging.simulate_message(_topic(CommandInbox.PING), _request(CommandInbox.PING))
    assert not h.messaging.published


def test_close_is_idempotent_and_start_after_close_is_a_noop(h):
    h.inbox.start()
    h.inbox.close()
    h.inbox.close()  # must not raise
    h.inbox.start()  # closed -> must not resubscribe
    assert not h.messaging.subscribed_topics()


def test_start_failure_disables_the_inbox_without_raising(h):
    def boom_subscribe(topic, callback, max_concurrency=None, max_messages=None):
        raise RuntimeError("broker unavailable")

    h.messaging.subscribe = boom_subscribe
    h.inbox.start()  # must not raise; the inbox self-disables
    assert not h.messaging.subscribed_topics()


# ===================== built-in verbs =====================


def test_ping_replies_status_and_uptime(h):
    h.uptime = 1234
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.PING), _request(CommandInbox.PING))
    body = h.only_reply_body()
    assert body["ok"] is True
    assert body["result"]["status"] == "RUNNING"
    assert body["result"]["uptimeSecs"] == 1234


def test_ping_is_answered_even_when_heartbeat_disabled(h):
    # The ping uptime source is fully injected - the inbox has no knowledge of
    # heartbeat.enabled, so it always answers regardless of that setting.
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.PING), _request(CommandInbox.PING))
    assert h.only_reply_body()["ok"] is True


def test_status_without_a_provider_answers_like_ping_and_omits_instances(h):
    # A plain service (a processor, a sink) reports no instances: `status` is then
    # byte-for-byte what `ping` replies - the section is omitted, not an empty array.
    h.uptime = 77
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.STATUS), _request(CommandInbox.STATUS))
    body = h.only_reply_body()
    assert body["ok"] is True
    assert body["result"] == {"status": "RUNNING", "uptimeSecs": 77}


def test_status_returns_the_provider_sample_including_state_and_attributes():
    # The pulled answer is the provider sample - the same one the state keepalive pushes.
    from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity

    sample = [
        InstanceConnectivity.of("cam-01", True).with_state("ONLINE"),
        InstanceConnectivity("cam-02", False, "connect timed out", "BACKOFF",
                             {"capabilities": ["ptz", "snapshot"],
                              "lastError": "CAMERA_UNAVAILABLE"}),
    ]
    h = Harness(instance_connectivity=lambda: sample)
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.STATUS), _request(CommandInbox.STATUS))

    result = h.only_reply_body()["result"]
    assert result["status"] == "RUNNING"
    instances = result["instances"]
    assert len(instances) == 2
    assert instances[0] == {"instance": "cam-01", "connected": True, "state": "ONLINE"}, (
        "an empty attribute bag (and an absent detail) is omitted"
    )
    assert instances[1] == {
        "instance": "cam-02",
        "connected": False,
        "state": "BACKOFF",
        "detail": "connect timed out",
        "attributes": {
            "capabilities": ["ptz", "snapshot"],
            "lastError": "CAMERA_UNAVAILABLE",
        },
    }


def test_status_survives_a_throwing_provider():
    # In production the source is EnhancedHeartbeat.sample_instance_connectivity, which
    # swallows a component's provider bug and yields an empty list (so `status` still
    # answers - see test_enhanced_heartbeat_unit). This asserts the inbox itself is safe
    # even when a caller wires a raw raising source: it degrades to the standard uncoded
    # -failure reply and never crashes.
    def boom():
        raise RuntimeError("provider blew up")

    h = Harness(instance_connectivity=boom)
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.STATUS), _request(CommandInbox.STATUS))
    body = h.only_reply_body()
    assert body["ok"] is False
    assert body["error"]["code"] == CommandInbox.ERR_HANDLER_ERROR


def test_status_through_the_heartbeat_seam_degrades_to_ping_when_the_provider_raises():
    # The production wiring: the inbox pulls the heartbeat's sampling seam, so a raising
    # component provider costs only the instances[] section - the verb still answers.
    from unittest.mock import MagicMock

    from edgecommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat

    heartbeat = EnhancedHeartbeat(MagicMock())

    def boom():
        raise RuntimeError("provider blew up")

    heartbeat.set_instance_connectivity_provider(boom)
    h = Harness(instance_connectivity=heartbeat.sample_instance_connectivity)
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.STATUS), _request(CommandInbox.STATUS))
    body = h.only_reply_body()
    assert body["ok"] is True
    assert "instances" not in body["result"]


def test_status_cannot_be_shadowed_by_a_custom_verb(h):
    with pytest.raises(ValueError):
        h.inbox.register(CommandInbox.STATUS, lambda request: None)
    with pytest.raises(ValueError):
        h.inbox.unregister(CommandInbox.STATUS)


def test_reply_carries_the_request_correlation_id_verb_name_and_responder_identity(h):
    h.inbox.start()
    ping = _request(CommandInbox.PING)
    h.messaging.simulate_message(_topic(CommandInbox.PING), ping)
    published = h.only_reply()
    assert published.message.get_header().correlation_id == ping.get_header().correlation_id, (
        "the reply must carry the request's correlation_id"
    )
    assert published.message.get_header().name == CommandInbox.PING, (
        "the reply header.name is the verb"
    )
    assert published.message.get_header().version == CommandInbox.CMD_MESSAGE_VERSION
    assert published.message.get_identity() is not None, (
        "the reply is config-stamped with the responder's identity"
    )
    assert published.message.get_identity().component == "TestComponent"


def test_reload_config_replies_ack_on_success(h):
    h.inbox.start()
    h.messaging.simulate_message(
        _topic(CommandInbox.RELOAD_CONFIG), _request(CommandInbox.RELOAD_CONFIG)
    )
    body = h.only_reply_body()
    assert body["ok"] is True
    assert body["result"]["reloaded"] is True


def test_reload_config_replies_reload_failed_on_failure(h):
    h.reload_ok = False
    h.inbox.start()
    h.messaging.simulate_message(
        _topic(CommandInbox.RELOAD_CONFIG), _request(CommandInbox.RELOAD_CONFIG)
    )
    body = h.only_reply_body()
    assert body["ok"] is False
    assert body["error"]["code"] == CommandInbox.ERR_RELOAD_FAILED
    assert body["error"]["message"]


def test_get_configuration_replies_the_redacted_effective_config(h):
    h.inbox.start()
    h.messaging.simulate_message(
        _topic(CommandInbox.GET_CONFIGURATION), _request(CommandInbox.GET_CONFIGURATION)
    )
    body = h.only_reply_body()
    assert body["ok"] is True
    assert body["result"]["config"] == h.redacted, (
        "get-configuration must return the redacted effective config (Flow B)"
    )


def test_get_configuration_replies_no_config_when_unavailable(h):
    h.redacted = None
    h.inbox.start()
    h.messaging.simulate_message(
        _topic(CommandInbox.GET_CONFIGURATION), _request(CommandInbox.GET_CONFIGURATION)
    )
    body = h.only_reply_body()
    assert body["ok"] is False
    assert body["error"]["code"] == CommandInbox.ERR_NO_CONFIG


def test_describe_includes_built_ins_custom_verbs_and_panels(h):
    h.inbox.register("sb/browse", lambda request: {"nodes": []})
    panel = {
        "id": "address-space",
        "title": "Address Space",
        "order": 20,
        "widgets": [{"kind": "treeBrowser", "browseVerb": "sb/browse"}],
    }
    h.inbox.register_panel(panel)
    panel["title"] = "mutated after registration"

    h.inbox.start()
    h.messaging.simulate_message(
        _topic(CommandInbox.DESCRIBE), _request(CommandInbox.DESCRIBE)
    )
    body = h.only_reply_body()

    assert body["ok"] is True
    result = body["result"]
    assert result["schemaVersion"] == "edgecommons.component.describe.v1"
    assert result["component"]["component"] == "TestComponent"
    assert result["component"]["instance"] == "main"
    assert result["component"]["path"] == "test-thing"
    verbs = {entry["verb"]: entry["builtIn"] for entry in result["commands"]}
    assert verbs[CommandInbox.PING] is True
    assert verbs[CommandInbox.DESCRIBE] is True
    assert verbs[CommandInbox.GET_CONFIGURATION] is True
    assert verbs[CommandInbox.RELOAD_CONFIG] is True
    assert verbs[CommandInbox.STATUS] is True
    assert verbs["sb/browse"] is False
    assert result["panels"]["schemaVersion"] == "edgecommons.panels.v2"
    assert result["panels"]["provider"] == "TestComponent"
    assert result["panels"]["renderer"] == "descriptor"
    assert result["panels"]["defaultView"] == "address-space"
    assert result["panels"]["views"] == [
        {
            "id": "address-space",
            "title": "Address Space",
            "order": 20,
            "widgets": [{"kind": "treeBrowser", "browseVerb": "sb/browse"}],
        }
    ]
    digest_payload = json.dumps(
        {"commands": result["commands"], "panels": result["panels"]},
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    assert result["digest"] == "sha256:" + hashlib.sha256(digest_payload).hexdigest()


# ===================== custom verbs (the registration seam) =====================


def test_custom_verb_registers_and_dispatches(h):
    h.inbox.start()  # registration after start needs no new subscription

    def handler(request):
        return {"restarted": True}

    h.inbox.register("restart-pipeline", handler)
    h.messaging.simulate_message(_topic("restart-pipeline"), _request("restart-pipeline"))
    body = h.only_reply_body()
    assert body["ok"] is True
    assert body["result"]["restarted"] is True


def test_namespaced_custom_verb_dispatches(h):
    h.inbox.register("sb/status", lambda request: None)  # None result -> empty ack
    h.inbox.start()
    h.messaging.simulate_message(_topic("sb/status"), _request("sb/status"))
    body = h.only_reply_body()
    assert body["ok"] is True
    assert body["result"] == {}, "a None handler result must reply an empty result object"


def test_handler_command_exception_keeps_its_code(h):
    def handler(request):
        raise CommandException("NOT_ALLOWED", "operator role required")

    h.inbox.register("guarded", handler)
    h.inbox.start()
    h.messaging.simulate_message(_topic("guarded"), _request("guarded"))
    body = h.only_reply_body()
    assert body["ok"] is False
    assert body["error"]["code"] == "NOT_ALLOWED"
    assert body["error"]["message"] == "operator role required"


def test_handler_uncoded_exception_maps_to_handler_error(h):
    def handler(request):
        raise ValueError("boom")

    h.inbox.register("boomy", handler)
    h.inbox.start()
    h.messaging.simulate_message(_topic("boomy"), _request("boomy"))
    body = h.only_reply_body()
    assert body["ok"] is False
    assert body["error"]["code"] == CommandInbox.ERR_HANDLER_ERROR


def test_register_rejects_shadowing_and_invalid_verbs(h):
    with pytest.raises(ValueError):
        h.inbox.register(CommandInbox.PING, lambda request: None)
    with pytest.raises(ValueError):
        h.inbox.register(CommandInbox.DESCRIBE, lambda request: None)
    with pytest.raises(ValueError):
        h.inbox.register(CommandInbox.SET_CONFIG_VERB, lambda request: None)
    h.inbox.register("mine", lambda request: None)
    with pytest.raises(ValueError):
        h.inbox.register("mine", lambda request: None)
    with pytest.raises(UnsValidationError):
        h.inbox.register("bad+verb", lambda request: None)
    with pytest.raises(UnsValidationError):
        h.inbox.register("sb//x", lambda request: None)  # empty namespace token


def test_unregister_removes_custom_verbs_but_never_built_ins(h):
    h.inbox.register("mine", lambda request: None)
    assert "mine" in h.inbox.verbs()
    h.inbox.unregister("mine")
    assert "mine" not in h.inbox.verbs()
    h.inbox.unregister("mine")  # unknown -> no-op, must not raise
    with pytest.raises(ValueError):
        h.inbox.unregister(CommandInbox.RELOAD_CONFIG)
    # The unregistered verb now gets the unknown-verb error.
    h.inbox.start()
    h.messaging.simulate_message(_topic("mine"), _request("mine"))
    assert h.only_reply_body()["error"]["code"] == CommandInbox.ERR_UNKNOWN_VERB


def test_register_rejects_none_verb_or_handler(h):
    with pytest.raises(ValueError):
        h.inbox.register(None, lambda request: None)
    with pytest.raises(ValueError):
        h.inbox.register("mine", None)


def test_unregister_rejects_none_verb(h):
    with pytest.raises(ValueError):
        h.inbox.unregister(None)


def test_verbs_snapshot_contains_built_ins_and_customs(h):
    h.inbox.register("mine", lambda request: None)
    assert h.inbox.verbs() == {
        CommandInbox.PING,
        CommandInbox.DESCRIBE,
        CommandInbox.RELOAD_CONFIG,
        CommandInbox.GET_CONFIGURATION,
        CommandInbox.STATUS,
        "mine",
    }


# ===================== descriptor panels =====================


def test_panel_registration_snapshot_contains_registered_panels(h):
    panel = {"id": "overview", "title": "Overview", "widgets": [{"kind": "summary"}]}
    h.inbox.register_panel(panel)

    snapshot = h.inbox.panels()
    assert snapshot == [
        {"id": "overview", "title": "Overview", "widgets": [{"kind": "summary"}]}
    ]

    panel["title"] = "changed"
    snapshot[0]["title"] = "also changed"
    assert h.inbox.panels() == [
        {"id": "overview", "title": "Overview", "widgets": [{"kind": "summary"}]}
    ]


@pytest.mark.parametrize(
    "panel",
    [
        None,
        [],
        "overview",
        {},
        {"id": "", "title": "Overview"},
        {"id": 1, "title": "Overview"},
        {"id": "overview"},
        {"id": "overview", "title": ""},
        {"id": "overview", "title": 1},
    ],
)
def test_register_panel_rejects_invalid_panels(h, panel):
    with pytest.raises(ValueError):
        h.inbox.register_panel(panel)


def test_register_panel_rejects_duplicate_ids(h):
    h.inbox.register_panel({"id": "overview", "title": "Overview"})
    with pytest.raises(ValueError):
        h.inbox.register_panel({"id": "overview", "title": "Duplicate"})


# ===================== unknown / fire-and-forget / malformed =====================


def test_unknown_verb_request_gets_an_unknown_verb_error_reply(h):
    h.inbox.start()
    h.messaging.simulate_message(_topic("no-such-verb"), _request("no-such-verb"))
    body = h.only_reply_body()
    assert body["ok"] is False
    assert body["error"]["code"] == CommandInbox.ERR_UNKNOWN_VERB


def test_unknown_fire_and_forget_verb_is_ignored(h):
    h.inbox.start()
    h.messaging.simulate_message(_topic("no-such-verb"), _notification("no-such-verb"))
    assert not h.messaging.published, "an unknown fire-and-forget verb must not be replied to"


def test_no_reply_to_runs_the_handler_without_replying(h):
    ran = []
    h.inbox.register("do-it", lambda request: ran.append(True))
    h.inbox.start()
    h.messaging.simulate_message(_topic("do-it"), _notification("do-it"))
    assert ran, "a fire-and-forget command must still run the handler"
    assert not h.messaging.published, "...but never reply"


def test_fire_and_forget_handler_failure_is_logged_only(h):
    def handler(request):
        raise CommandException("NOPE", "nope")

    h.inbox.register("do-it", handler)
    h.inbox.start()
    h.messaging.simulate_message(_topic("do-it"), _notification("do-it"))  # must not raise
    assert not h.messaging.published


def test_fire_and_forget_uncoded_handler_failure_is_logged_only(h):
    def handler(request):
        raise ValueError("boom")

    h.inbox.register("do-it", handler)
    h.inbox.start()
    h.messaging.simulate_message(_topic("do-it"), _notification("do-it"))  # must not raise
    assert not h.messaging.published


def test_handle_swallows_an_exception_from_a_malformed_message(h):
    class ExplodingMessage:
        def get_header(self):
            raise RuntimeError("corrupt payload")

    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.PING), ExplodingMessage())  # must not raise
    assert not h.messaging.published


def test_handle_ignores_a_delivery_with_an_empty_verb(h):
    # ".../cmd/#" also matches the exact ".../cmd/" level (an empty trailing verb) -
    # nothing to dispatch there either.
    h.inbox.start()
    h.inbox._handle(INBOX_FILTER[:-1], _request(CommandInbox.PING))
    assert not h.messaging.published


def test_handle_is_a_noop_when_closed_mid_flight(h):
    # The race the internal `closed` check under the lock guards: a message that
    # slipped through before unsubscribe took effect must still not dispatch.
    h.inbox.start()
    h.inbox.close()
    h.inbox._handle(_topic(CommandInbox.PING), _request(CommandInbox.PING))
    assert not h.messaging.published


def test_close_swallows_an_unsubscribe_failure(h):
    def boom_unsubscribe(topic):
        raise RuntimeError("already gone")

    h.inbox.start()
    h.messaging.unsubscribe = boom_unsubscribe
    h.inbox.close()  # must not raise despite the unsubscribe call failing


def test_malformed_payloads_are_ignored_without_reply_and_never_crash(h):
    h.inbox.start()
    # header.name does not equal the topic verb (foreign convention on a cmd topic).
    h.messaging.simulate_message(_topic(CommandInbox.PING), _request("something-else"))
    # A raw (headerless) envelope - junk JSON on the inbox.
    h.messaging.simulate_message(_topic(CommandInbox.PING), Message.from_object({}))
    # A None message must not crash the callback either.
    h.messaging.simulate_message(_topic(CommandInbox.PING), None)
    assert not h.messaging.published, "malformed/foreign payloads must never be replied to"


def test_delegated_set_config_is_ignored_even_as_a_request(h):
    h.inbox.start()
    h.messaging.simulate_message(
        _topic(CommandInbox.SET_CONFIG_VERB), _request(CommandInbox.SET_CONFIG_VERB)
    )
    assert not h.messaging.published, (
        "set-config is owned by the CONFIG_COMPONENT subscription - never dispatched or"
        " replied to here"
    )


def test_bare_cmd_parent_level_delivery_is_ignored(h):
    h.inbox.start()
    # MQTT "#" also matches the parent level (".../cmd") - nothing to dispatch there.
    h.messaging.simulate_message(
        "ecv1/test-thing/TestComponent/main/cmd", _request(CommandInbox.PING)
    )
    assert not h.messaging.published


def test_a_failing_reply_publish_is_swallowed(h):
    def boom_reply(request, reply):
        raise RuntimeError("broker down")

    h.messaging.reply = boom_reply
    h.inbox.start()
    h.messaging.simulate_message(_topic(CommandInbox.PING), _request(CommandInbox.PING))  # no raise
    h.inbox.close()


# ===================== constructor validation =====================


class TestConstructorValidation:
    def test_none_config_raises(self):
        with pytest.raises(ValueError):
            CommandInbox(None, FakeMessaging(), lambda: 0, lambda: True, lambda: None)

    def test_none_broker_client_raises(self):
        with pytest.raises(ValueError):
            CommandInbox(FakeConfig(), None, lambda: 0, lambda: True, lambda: None)

    def test_none_uptime_secs_raises(self):
        with pytest.raises(ValueError):
            CommandInbox(FakeConfig(), FakeMessaging(), None, lambda: True, lambda: None)

    def test_none_config_reload_raises(self):
        with pytest.raises(ValueError):
            CommandInbox(FakeConfig(), FakeMessaging(), lambda: 0, None, lambda: None)

    def test_none_redacted_config_raises(self):
        with pytest.raises(ValueError):
            CommandInbox(FakeConfig(), FakeMessaging(), lambda: 0, lambda: True, None)


class TestCommandException:
    def test_empty_code_raises(self):
        with pytest.raises(ValueError):
            CommandException("", "message")

    def test_code_and_message_are_accessible(self):
        e = CommandException("MY_CODE", "my message")
        assert e.code == "MY_CODE"
        assert e.message == "my message"


# ===================== commands.json conformance =====================

VECTORS_DIR = Path(__file__).resolve().parents[3] / "uns-test-vectors"
_COMMANDS_JSON = VECTORS_DIR / "commands.json"


def _load_commands_vectors():
    return json.loads(_COMMANDS_JSON.read_text(encoding="utf-8"))


@pytest.mark.skipif(not _COMMANDS_JSON.exists(), reason="uns-test-vectors not present")
class TestCommandsJsonConformance:
    """Replays ``uns-test-vectors/commands.json`` through a live CommandInbox: the
    inbox filter byte-for-byte, the built-in verb request/reply goldens (reply
    bodies must equal the live dispatch's output, D-U22 - the envelope's uuid/
    timestamp are inherently non-reproducible via a live dispatch and are not
    compared), the UNKNOWN_VERB case, and the behavior/builtInVerbs/errorCodes
    constants."""

    @staticmethod
    def _vectors():
        return _load_commands_vectors()

    @staticmethod
    def _identity(inp):
        return MessageIdentity(
            [HierEntry("device", inp["device"])], inp["component"], inp["instance"]
        )

    def _harness_for(self, vectors):
        inp = vectors["inbox"]["input"]
        config = FakeConfig(include_root=inp["includeRoot"])
        config.set_component_identity(self._identity(inp))
        return config

    def test_inbox_filter_matches_byte_for_byte(self):
        vectors = self._vectors()
        config = self._harness_for(vectors)
        messaging = FakeMessaging()
        inbox = CommandInbox(config, messaging, lambda: 42, lambda: True, lambda: None)
        inbox.start()
        assert messaging.subscribed_topics() == {vectors["inbox"]["filter"]}

    def _dispatch_case(self, case, vectors, *, uptime=42, reload_ok=True, redacted=None):
        config = self._harness_for(vectors)
        messaging = FakeMessaging()
        inbox = CommandInbox(
            config, messaging, lambda: uptime, lambda: reload_ok, lambda: redacted
        )
        inbox.start()
        request = MessageBuilder.from_object(case["request"]).build()
        messaging.simulate_message(case["topic"], request)
        assert len(messaging.published) == 1, f"'{case['name']}' must reply exactly once"
        published = messaging.published[0]
        assert published.topic == case["request"]["header"]["reply_to"], (
            f"'{case['name']}' reply topic"
        )
        return published.message

    def test_ping_golden_replayed_through_a_live_inbox(self):
        vectors = self._vectors()
        case = next(c for c in vectors["verbs"] if c["name"] == "ping")
        reply = self._dispatch_case(case, vectors, uptime=42)
        self._assert_reply_matches_golden(reply, case)

    def test_reload_config_golden_replayed_through_a_live_inbox(self):
        vectors = self._vectors()
        case = next(c for c in vectors["verbs"] if c["name"] == "reload-config")
        reply = self._dispatch_case(case, vectors, reload_ok=True)
        self._assert_reply_matches_golden(reply, case)

    def test_get_configuration_golden_replayed_through_a_live_inbox(self):
        vectors = self._vectors()
        case = next(c for c in vectors["verbs"] if c["name"] == "get-configuration")
        golden_config = case["reply"]["body"]["result"]["config"]
        reply = self._dispatch_case(case, vectors, redacted=golden_config)
        self._assert_reply_matches_golden(reply, case)

    def test_describe_golden_replayed_through_a_live_inbox(self):
        vectors = self._vectors()
        case = next(c for c in vectors["verbs"] if c["name"] == "describe")
        golden_config = next(
            c for c in vectors["verbs"] if c["name"] == "get-configuration"
        )["reply"]["body"]["result"]["config"]
        reply = self._dispatch_case(case, vectors, redacted=golden_config)
        self._assert_reply_matches_golden(reply, case)

    def test_status_golden_replayed_through_a_live_inbox(self):
        # The golden case pins the no-provider component (a plain service): the reply is
        # ping's body and the instances[] section is omitted, not an empty array.
        vectors = self._vectors()
        case = next(c for c in vectors["verbs"] if c["name"] == "status")
        reply = self._dispatch_case(case, vectors, uptime=42)
        self._assert_reply_matches_golden(reply, case)

    def test_unknown_verb_golden_replayed_through_a_live_inbox(self):
        vectors = self._vectors()
        case = next(c for c in vectors["errors"] if c["name"] == "unknown-verb")
        reply = self._dispatch_case(case, vectors)
        self._assert_reply_matches_golden(reply, case)

    @staticmethod
    def _assert_reply_matches_golden(reply, case):
        golden = case["reply"]
        assert reply.get_header().name == golden["header"]["name"], f"'{case['name']}' name"
        assert reply.get_header().version == golden["header"]["version"], (
            f"'{case['name']}' version"
        )
        assert reply.get_header().correlation_id == case["request"]["header"]["correlation_id"], (
            f"'{case['name']}' correlation_id must equal the request's"
        )
        assert reply.get_identity() is not None
        assert reply.get_identity().to_dict() == golden["identity"], (
            f"'{case['name']}' responder identity"
        )
        assert reply.get_body() == golden["body"], (
            f"'{case['name']}' reply body must equal a live inbox dispatch's output"
        )

    def test_behavior_and_verb_set_constants(self):
        vectors = self._vectors()
        behavior = vectors["behavior"]
        assert behavior["verbIsTopicChannel"] is True
        assert behavior["headerNameMustEqualVerb"] is True
        assert behavior["fireAndForgetWithoutReplyTo"] is True
        assert behavior["malformedIgnoredWithoutReply"] is True
        assert set(behavior["builtInVerbs"]) == CommandInbox.BUILT_IN_VERBS
        assert set(behavior["delegatedVerbs"]) == CommandInbox.DELEGATED_VERBS
        assert set(behavior["errorCodes"]) == {
            CommandInbox.ERR_UNKNOWN_VERB,
            CommandInbox.ERR_HANDLER_ERROR,
            CommandInbox.ERR_RELOAD_FAILED,
            CommandInbox.ERR_NO_CONFIG,
        }
