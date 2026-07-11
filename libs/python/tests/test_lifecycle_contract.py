"""Adversarial P1 configuration/readiness/command lifecycle contract tests."""

import threading
import time
import json
from types import SimpleNamespace

import pytest

from edgecommons import (
    CommandInboxStartupState,
    ConfigurationCandidateRejected,
    ConfigurationValidationPhase,
    ConfigurationValidationResult,
    EdgeCommonsBuilder,
)
from edgecommons.command_inbox import CommandInbox
from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.shadow_config_manager import ShadowConfigManager
from edgecommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from edgecommons.health import ReadinessState
from edgecommons.messaging.messaging_client import MessagingClient
from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.messaging.message_builder import MessageBuilder


class _MutableConfigManager(ConfigManager):
    def __init__(self, config, **kwargs):
        self.source = config
        super().__init__("com.example.Camera", "camera-host", **kwargs)
        self.init()

    def _load_configuration(self):
        return self.source


def _config(value, **extra):
    config = {"component": {"global": {"value": value}, "instances": []}}
    config.update(extra)
    return config


def test_initial_validator_inputs_are_defensive_and_independent():
    seen = []

    def mutator(candidate, current, phase):
        assert current is None
        assert phase is ConfigurationValidationPhase.INITIAL
        candidate["component"]["global"]["value"] = "mutated"
        return ConfigurationValidationResult.accept()

    def observer(candidate, current, phase):
        seen.append(candidate["component"]["global"]["value"])
        return ConfigurationValidationResult.accept()

    manager = _MutableConfigManager(
        _config("original"),
        candidate_validators={"mutator": mutator, "observer": observer},
    )

    assert seen == ["original"]
    assert manager.get_global_config() == {"value": "original"}
    returned = manager.get_effective_config()
    returned["component"]["global"]["value"] = "external mutation"
    assert manager.get_global_config() == {"value": "original"}
    assert manager.get_generation() == 1


def test_reload_rejection_keeps_exact_prior_generation_and_skips_listeners():
    def validator(candidate, current, phase):
        if phase is ConfigurationValidationPhase.RELOAD:
            return ConfigurationValidationResult.reject("CAMERA_INVALID", "bad camera")
        return ConfigurationValidationResult.accept()

    manager = _MutableConfigManager(
        _config(1), candidate_validators={"camera": validator}
    )
    manager.complete_initialization()
    notifications = []

    class Listener(ConfigurationChangeListener):
        def on_configuration_change(self, configuration):
            notifications.append(configuration)
            return True

    manager.add_config_change_listener(Listener())
    prior = manager.get_effective_config()
    assert manager.configuration_changed(_config(2)) is False
    assert manager.get_effective_config() == prior
    assert manager.get_generation() == 1
    assert notifications == []
    assert manager.get_last_candidate_validation_errors()[0].code == "CAMERA_INVALID"


def test_validator_gets_redacted_prior_and_cannot_mutate_current():
    seen = []

    def validator(candidate, current, phase):
        if phase is ConfigurationValidationPhase.RELOAD:
            seen.append(current["messaging"]["local"]["credentials"])
            current["messaging"]["local"]["credentials"] = "changed"
        return ConfigurationValidationResult.accept()

    manager = _MutableConfigManager(
        _config(
            1,
            messaging={
                "local": {"credentials": {"username": "camera", "password": "secret"}}
            },
        ),
        validate_config=False,
        candidate_validators={"camera": validator},
    )
    assert manager.configuration_changed(_config(2)) is True
    assert seen == ["***"]
    prior = manager.get_effective_config()
    # The callback saw a separate redacted copy; accepted generation 1 was never altered.
    assert prior == _config(2)


def test_reload_timeout_is_one_overall_deadline_and_late_callback_cannot_commit():
    release = threading.Event()
    finished = threading.Event()

    def slow(candidate, current, phase):
        try:
            if phase is ConfigurationValidationPhase.RELOAD:
                release.wait(2)
            return ConfigurationValidationResult.accept()
        finally:
            if phase is ConfigurationValidationPhase.RELOAD:
                finished.set()

    manager = _MutableConfigManager(
        _config(1),
        candidate_validators={"slow": slow},
        validation_timeout_secs=0.05,
    )
    started = time.monotonic()
    try:
        assert manager.configuration_changed(_config(2)) is False
        assert time.monotonic() - started < 0.5
        assert manager.get_generation() == 1
        assert manager.get_global_config() == {"value": 1}
        error = manager.get_last_candidate_validation_errors()[0]
        assert error.code == "VALIDATION_TIMEOUT"
    finally:
        release.set()
        assert finished.wait(1)


def test_repeated_validator_timeouts_never_exceed_global_worker_cap():
    release = threading.Event()
    lock = threading.Lock()
    entered = 0
    live = 0
    max_live = 0

    def slow(candidate, current, phase):
        nonlocal entered, live, max_live
        if phase is ConfigurationValidationPhase.INITIAL:
            return ConfigurationValidationResult.accept()
        with lock:
            entered += 1
            live += 1
            max_live = max(max_live, live)
        try:
            release.wait(2)
            return ConfigurationValidationResult.accept()
        finally:
            with lock:
                live -= 1

    manager = _MutableConfigManager(
        _config(1),
        candidate_validators={"slow": slow},
        validation_timeout_secs=0.01,
    )
    try:
        for value in range(2, 14):
            assert manager.configuration_changed(_config(value)) is False
            assert manager.get_last_candidate_validation_errors()[0].code == (
                "VALIDATION_TIMEOUT"
            )

        with lock:
            assert entered == 4
            assert max_live == 4
            assert live == 4
        assert manager.get_generation() == 1
        assert manager.get_global_config() == {"value": 1}
    finally:
        release.set()
        deadline = time.monotonic() + 1
        while time.monotonic() < deadline:
            with lock:
                if live == 0:
                    break
            time.sleep(0.005)
        with lock:
            assert live == 0


def test_initial_rejection_prevents_file_watcher_start(tmp_path, monkeypatch):
    from edgecommons.config.manager.file_config_manager import FileConfigManager

    path = tmp_path / "config.json"
    path.write_text('{"component":{"global":{},"instances":[]}}')
    starts = []
    monkeypatch.setattr(
        "edgecommons.config.manager.file_config_manager.Observer.start",
        lambda self: starts.append(True),
    )

    with pytest.raises(ConfigurationCandidateRejected):
        FileConfigManager(
            "camera-host",
            "com.example.Camera",
            str(path),
            candidate_validators={
                "camera": lambda candidate, current, phase: (
                    ConfigurationValidationResult.reject("NO_CAMERA", "none enabled")
                )
            },
        )
    assert starts == []


def test_validator_and_listener_reentrant_activation_is_rejected():
    holder = {}
    nested_from_validator = []

    def validator(candidate, current, phase):
        if phase is ConfigurationValidationPhase.RELOAD:
            nested_from_validator.append(
                holder["manager"].configuration_changed(_config(99))
            )
        return ConfigurationValidationResult.accept()

    manager = _MutableConfigManager(
        _config(1), candidate_validators={"camera": validator}
    )
    holder["manager"] = manager
    manager.complete_initialization()
    nested_from_listener = []

    class Listener(ConfigurationChangeListener):
        def on_configuration_change(self, configuration):
            nested_from_listener.append(manager.configuration_changed(_config(100)))
            return True

    manager.add_config_change_listener(Listener())
    assert manager.configuration_changed(_config(2)) is True
    assert nested_from_validator == [False]
    assert nested_from_listener == [False]
    assert manager.get_global_config() == {"value": 2}
    assert manager.get_generation() == 2


def test_validation_timeout_hard_maximum_and_builder_forwards_lifecycle(monkeypatch):
    with pytest.raises(ValueError, match="60"):
        EdgeCommonsBuilder.create("x").configuration_validation_timeout(61)

    captured = {}

    def fake_edgecommons(**kwargs):
        captured.update(kwargs)
        return object()

    import edgecommons

    monkeypatch.setattr(edgecommons, "EdgeCommons", fake_edgecommons)
    validator = lambda candidate, current, phase: ConfigurationValidationResult.accept()
    configurer = lambda inbox: None
    EdgeCommonsBuilder.create("camera").initial_ready(False).configuration_validator(
        "camera", validator
    ).configuration_validation_timeout(3).configure_commands(configurer).build()

    assert captured["initial_ready"] is False
    assert captured["configuration_validators"] == {"camera": validator}
    assert captured["configuration_validation_timeout"] == 3.0
    assert captured["command_configurers"] == [configurer]


class _CommandConfig:
    def __init__(self):
        self.identity = MessageIdentity(
            [HierEntry("device", "camera-host")], "camera-adapter", "main"
        )

    def get_component_identity(self):
        return self.identity

    def is_topic_include_root(self):
        return False

    def get_tag_config(self):
        return None


class _LifecycleMessaging:
    def __init__(self):
        self.callback = None
        self.callbacks = []
        self.unsubscribed = []
        self.deliver_on_subscribe = []
        self.failure = None
        self.block_ack = False
        self.ack_entered = threading.Event()
        self.ack_release = threading.Event()

    def subscribe_acknowledged(
        self,
        topic,
        callback,
        max_concurrency=None,
        max_messages=None,
        timeout_secs=10.0,
    ):
        self.callback = callback
        self.callbacks.append(callback)
        self.ack_entered.set()
        for delivery in self.deliver_on_subscribe:
            callback(*delivery)
        if self.block_ack:
            self.ack_release.wait(timeout_secs)
        if self.failure is not None:
            raise self.failure

    def unsubscribe(self, topic):
        self.unsubscribed.append(topic)
        self.callback = None

    def reply(self, request, response):
        pass


def _command_message(seq):
    return MessageBuilder.create("work", "1.0").with_payload({"seq": seq}).build()


def _command_topic():
    return "ecv1/camera-host/camera-adapter/main/cmd/work"


def _inbox(messaging, handler):
    inbox = CommandInbox(
        _CommandConfig(), messaging, lambda: 1, lambda: True, lambda: {}
    )
    inbox.register("work", handler)
    return inbox


def _wait_until(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.005)
    assert predicate()


def test_command_activation_gate_preserves_pre_ack_messages_in_order():
    messaging = _LifecycleMessaging()
    observed = []
    messaging.deliver_on_subscribe = [
        (_command_topic(), _command_message(index)) for index in range(6)
    ]
    inbox = _inbox(messaging, lambda request: observed.append(request.get_body()["seq"]))

    status = inbox.start()
    assert status.state is CommandInboxStartupState.ACTIVE
    _wait_until(lambda: len(observed) == 6)
    assert observed == list(range(6))
    inbox.close()


def test_command_activation_gate_drops_newest_after_strict_256_bound():
    messaging = _LifecycleMessaging()
    observed = []
    messaging.deliver_on_subscribe = [
        (_command_topic(), _command_message(index)) for index in range(257)
    ]
    inbox = _inbox(messaging, lambda request: observed.append(request.get_body()["seq"]))

    assert inbox.start().state is CommandInboxStartupState.ACTIVE
    _wait_until(lambda: len(observed) == 256)
    assert observed == list(range(256))
    inbox.close()


def test_command_activation_drain_orders_concurrent_arrival_after_retained_batch():
    messaging = _LifecycleMessaging()
    observed = []
    first_entered = threading.Event()
    release_first = threading.Event()

    def handler(request):
        seq = request.get_body()["seq"]
        if seq == 1:
            first_entered.set()
            release_first.wait(2)
        observed.append(seq)

    messaging.deliver_on_subscribe = [
        (_command_topic(), _command_message(1)),
        (_command_topic(), _command_message(2)),
    ]
    inbox = _inbox(messaging, handler)
    assert inbox.start().state is CommandInboxStartupState.ACTIVE
    assert first_entered.wait(1)
    messaging.callbacks[-1](_command_topic(), _command_message(3))
    release_first.set()
    _wait_until(lambda: len(observed) == 3)
    assert observed == [1, 2, 3]
    inbox.close()


def test_stop_during_ack_invalidates_generation_and_allows_clean_restart():
    messaging = _LifecycleMessaging()
    messaging.block_ack = True
    observed = []
    inbox = _inbox(messaging, lambda request: observed.append(request.get_body()["seq"]))
    result = []
    thread = threading.Thread(target=lambda: result.append(inbox.start(1.0)))
    thread.start()
    assert messaging.ack_entered.wait(1)
    stale_callback = messaging.callbacks[-1]
    inbox.stop()
    messaging.ack_release.set()
    thread.join(1)

    assert result[0].state is CommandInboxStartupState.STOPPED
    stale_callback(_command_topic(), _command_message(1))
    assert observed == []

    messaging.block_ack = False
    messaging.ack_entered.clear()
    assert inbox.start().state is CommandInboxStartupState.ACTIVE
    messaging.callbacks[-1](_command_topic(), _command_message(2))
    assert observed == [2]
    inbox.close()


def test_ack_failure_is_failed_sanitized_cleaned_and_retryable():
    messaging = _LifecycleMessaging()
    messaging.failure = RuntimeError(
        "mqtt://user:password@host password=topsecret\ntransport down"
    )
    inbox = _inbox(messaging, lambda request: None)

    failed = inbox.start()
    assert failed.state is CommandInboxStartupState.FAILED
    assert "topsecret" not in failed.error
    assert "user:password" not in failed.error
    assert messaging.unsubscribed

    messaging.failure = None
    active = inbox.start()
    assert active.state is CommandInboxStartupState.ACTIVE
    inbox.close()


def test_readiness_requires_app_gate_connection_and_active_command_plane():
    connected = True
    command_active = False
    readiness = ReadinessState(
        lambda: connected,
        initial_ready=False,
        required_ready_fn=lambda: command_active,
    )
    assert readiness.is_ready() is False
    readiness.set_ready(True)
    assert readiness.is_ready() is False
    command_active = True
    assert readiness.is_ready() is True
    connected = False
    assert readiness.is_ready() is False
    connected = True
    readiness.set_shutting_down()
    assert readiness.is_ready() is False


def test_shadow_source_reports_only_committed_configuration_generations(monkeypatch):
    """A Shadow candidate cannot become reported/current until it has committed."""

    reports = []
    subscriptions = []
    initial = _config("initial")
    monkeypatch.setattr(
        MessagingClient, "get_native_client", staticmethod(lambda: object())
    )
    monkeypatch.setattr(
        ShadowConfigManager, "_get_configuration", lambda _self: initial
    )
    monkeypatch.setattr(
        ShadowConfigManager,
        "_report_updated_configuration",
        lambda _self, config: reports.append(config),
    )
    monkeypatch.setattr(
        ShadowConfigManager,
        "_subscribe_to_shadow_topics",
        lambda _self: subscriptions.append(True),
    )

    def reject_bad_candidates(candidate, _current, _phase):
        if candidate["component"]["global"]["value"] == "rejected":
            return ConfigurationValidationResult.reject("CAMERA_INVALID", "bad camera")
        return ConfigurationValidationResult.accept()

    # INITIAL validation raises before either the Shadow report or the provider
    # subscription can start.
    with pytest.raises(ConfigurationCandidateRejected):
        ShadowConfigManager(
            "camera-host",
            "com.example.Camera",
            "camera",
            candidate_validators={
                "camera": lambda _candidate, _current, _phase: (
                    ConfigurationValidationResult.reject("CAMERA_INVALID", "bad camera")
                )
            },
        )
    assert reports == []
    assert subscriptions == []

    manager = ShadowConfigManager(
        "camera-host",
        "com.example.Camera",
        "camera",
        candidate_validators={"camera": reject_bad_candidates},
    )
    manager.complete_initialization()
    assert manager.get_generation() == 1
    assert manager.get_effective_config() == initial
    assert reports == [initial]
    assert subscriptions == [True]

    listener_calls = []

    class Listener(ConfigurationChangeListener):
        def on_configuration_change(self, configuration):
            listener_calls.append(configuration)
            return True

    manager.add_config_change_listener(Listener())

    def delta(value):
        return SimpleNamespace(
            binary_message=SimpleNamespace(
                message=json.dumps(
                    {"state": {"ComponentConfig": json.dumps(_config(value))}}
                ).encode("utf-8"),
                context=SimpleNamespace(
                    topic="$aws/things/camera/shadow/update/delta"
                ),
            )
        )

    manager._on_shadow_event(delta("rejected"))
    assert manager.get_generation() == 1
    assert manager.get_effective_config() == initial
    assert listener_calls == []
    assert reports == [initial]

    manager._on_shadow_event(delta("accepted"))
    assert manager.get_generation() == 2
    assert manager.get_effective_config() == _config("accepted")
    assert listener_calls == [_config("accepted")]
    assert reports == [initial, _config("accepted")]
