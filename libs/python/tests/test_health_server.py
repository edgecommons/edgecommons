"""
Unit tests for the Phase 1c health slice (FR-HB-1 / FR-HB-2):

* ``/livez`` returns 200 and does NOT depend on the broker (200 even when messaging is disconnected);
* ``/readyz`` returns 503 before ready / when disconnected / after set_ready(False) / when shutting
  down, and 200 only when connected && ready && !shuttingDown;
* ``/startupz`` mirrors readiness;
* unknown paths return 404;
* the server is OFF by default on HOST/GREENGRASS and ON by default on KUBERNETES, and respects an
  explicit ``health.enabled``;
* SIGTERM/shutdown flips readiness to 503 and unsubscribes all tracked subscriptions (idempotent);
* ``HealthConfiguration`` parsing + the ``profile_health_enabled`` resolver helper.

Real loopback GETs are issued against an ephemeral port (``port: 0``). Mirrors the canonical Java
behavior; kept parallel to the other Phase-1c parity tests.
"""

import argparse
import http.client
import signal

import pytest

from ggcommons.config.health_config import HealthConfiguration
from ggcommons.health import HealthServer, ReadinessState
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.platform import Platform, profile_health_enabled


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------


class _ConnFlag:
    """A toggleable connected() source for ReadinessState."""

    def __init__(self, value=True):
        self.value = value

    def __call__(self):
        return self.value


def _get(port, path):
    """Issue a real loopback GET; return (status, body)."""
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("GET", path)
        resp = conn.getresponse()
        body = resp.read().decode("utf-8")
        return resp.status, body
    finally:
        conn.close()


@pytest.fixture
def server_factory():
    """Start a HealthServer on an ephemeral port; auto-stop all started servers."""
    started = []

    def _start(readiness, config=None):
        cfg = config or HealthConfiguration({"port": 0})
        srv = HealthServer(cfg, readiness, bind_host="127.0.0.1")
        srv.start()
        started.append(srv)
        return srv

    yield _start

    for srv in started:
        srv.stop()


# ---------------------------------------------------------------------------
# FR-HB-1: /livez is independent of the broker
# ---------------------------------------------------------------------------


def test_livez_200_even_when_disconnected(server_factory):
    # connected() is False -> liveness must STILL be 200 (process is alive).
    readiness = ReadinessState(_ConnFlag(False))
    srv = server_factory(readiness)
    status, body = _get(srv.port, "/livez")
    assert status == 200
    assert body == "ok"


def test_livez_200_when_shutting_down(server_factory):
    # Even during shutdown, the process is still alive => livez 200 (only readyz flips).
    readiness = ReadinessState(_ConnFlag(True))
    readiness.set_shutting_down()
    srv = server_factory(readiness)
    status, _ = _get(srv.port, "/livez")
    assert status == 200


# ---------------------------------------------------------------------------
# FR-HB-1: /readyz reflects connected && ready && !shuttingDown
# ---------------------------------------------------------------------------


def test_readyz_503_when_disconnected(server_factory):
    readiness = ReadinessState(_ConnFlag(False))
    srv = server_factory(readiness)
    status, body = _get(srv.port, "/readyz")
    assert status == 503
    assert body == "not ready"


def test_readyz_200_when_connected_and_ready(server_factory):
    readiness = ReadinessState(_ConnFlag(True))  # ready flag defaults True
    srv = server_factory(readiness)
    status, body = _get(srv.port, "/readyz")
    assert status == 200
    assert body == "ok"


def test_readyz_503_after_set_ready_false(server_factory):
    flag = _ConnFlag(True)
    readiness = ReadinessState(flag)
    srv = server_factory(readiness)
    # Connected + ready => 200
    assert _get(srv.port, "/readyz")[0] == 200
    # App gates readiness off => 503 even though still connected
    readiness.set_ready(False)
    assert _get(srv.port, "/readyz")[0] == 503
    # And back on => 200
    readiness.set_ready(True)
    assert _get(srv.port, "/readyz")[0] == 200


def test_readyz_503_when_shutting_down(server_factory):
    readiness = ReadinessState(_ConnFlag(True))
    srv = server_factory(readiness)
    assert _get(srv.port, "/readyz")[0] == 200
    readiness.set_shutting_down()
    assert _get(srv.port, "/readyz")[0] == 503


def test_readiness_state_logic_table():
    """is_ready() == connected && ready && !shuttingDown, exhaustively."""
    flag = _ConnFlag(True)
    r = ReadinessState(flag)
    assert r.is_ready() is True
    flag.value = False
    assert r.is_ready() is False
    flag.value = True
    r.set_ready(False)
    assert r.is_ready() is False
    r.set_ready(True)
    assert r.is_ready() is True
    r.set_shutting_down()
    assert r.is_ready() is False
    # shutting_down latches even if connected/ready flip back
    flag.value = True
    r.set_ready(True)
    assert r.is_ready() is False


def test_readiness_connected_fn_raising_is_not_ready():
    def boom():
        raise RuntimeError("messaging blew up")

    r = ReadinessState(boom)
    assert r.is_ready() is False  # fail closed


# ---------------------------------------------------------------------------
# FR-HB-1: /startupz mirrors readiness
# ---------------------------------------------------------------------------


def test_startupz_mirrors_readiness(server_factory):
    flag = _ConnFlag(False)
    readiness = ReadinessState(flag)
    srv = server_factory(readiness)
    assert _get(srv.port, "/startupz")[0] == 503
    flag.value = True
    assert _get(srv.port, "/startupz")[0] == 200
    readiness.set_shutting_down()
    assert _get(srv.port, "/startupz")[0] == 503


# ---------------------------------------------------------------------------
# unknown path -> 404
# ---------------------------------------------------------------------------


def test_unknown_path_404(server_factory):
    readiness = ReadinessState(_ConnFlag(True))
    srv = server_factory(readiness)
    status, body = _get(srv.port, "/nope")
    assert status == 404
    assert body == "not found"


def test_custom_paths(server_factory):
    readiness = ReadinessState(_ConnFlag(True))
    cfg = HealthConfiguration(
        {
            "port": 0,
            "livenessPath": "/alive",
            "readinessPath": "/ready",
            "startupPath": "/start",
        }
    )
    srv = server_factory(readiness, cfg)
    assert _get(srv.port, "/alive")[0] == 200
    assert _get(srv.port, "/ready")[0] == 200
    assert _get(srv.port, "/start")[0] == 200
    # The defaults are no longer routed when custom paths are configured.
    assert _get(srv.port, "/livez")[0] == 404


# ---------------------------------------------------------------------------
# HealthConfiguration parsing
# ---------------------------------------------------------------------------


def test_health_config_defaults():
    hc = HealthConfiguration(None)
    assert hc.enabled is None  # not specified -> let the platform default decide
    assert hc.port == 8081
    assert hc.liveness_path == "/livez"
    assert hc.readiness_path == "/readyz"
    assert hc.startup_path == "/startupz"


def test_health_config_explicit():
    hc = HealthConfiguration(
        {
            "enabled": False,
            "port": 9000,
            "livenessPath": "/l",
            "readinessPath": "/r",
            "startupPath": "/s",
        }
    )
    assert hc.enabled is False
    assert hc.port == 9000
    assert hc.liveness_path == "/l"
    assert hc.readiness_path == "/r"
    assert hc.startup_path == "/s"


def test_health_config_enabled_true():
    assert HealthConfiguration({"enabled": True}).enabled is True


# ---------------------------------------------------------------------------
# FR-RT-3: default-on-KUBERNETES, opt-in elsewhere, explicit override
# ---------------------------------------------------------------------------


def test_profile_health_enabled_helper():
    assert profile_health_enabled(Platform.KUBERNETES) is True
    assert profile_health_enabled(Platform.HOST) is False
    assert profile_health_enabled(Platform.GREENGRASS) is False
    assert profile_health_enabled(None) is False


# ---- GGCommons._init_health enablement (without a full broker-backed build) ----

from ggcommons.ggcommons import GGCommons  # noqa: E402


class _FakeConfigManager:
    def __init__(self, health_json):
        self._hc = HealthConfiguration(health_json)

    def get_health_config(self):
        return self._hc


def _init_health_for(health_json, platform):
    """Drive GGCommons._init_health on a bare instance (bypassing the broker-backed __init__)."""
    gg = GGCommons.__new__(GGCommons)
    gg._readiness = None
    gg._health_server = None
    gg._config_manager = _FakeConfigManager(health_json)
    ns = argparse.Namespace(platform=platform)
    gg._init_health(ns)
    return gg


def test_health_off_by_default_on_host():
    gg = _init_health_for({"port": 0}, Platform.HOST)
    try:
        assert gg._health_server is None  # not started
        assert gg._readiness is not None  # readiness always built (set_ready works)
    finally:
        if gg._health_server:
            gg._health_server.stop()


def test_health_off_by_default_on_greengrass():
    gg = _init_health_for({"port": 0}, Platform.GREENGRASS)
    try:
        assert gg._health_server is None
    finally:
        if gg._health_server:
            gg._health_server.stop()


def test_health_on_by_default_on_kubernetes():
    gg = _init_health_for({"port": 0}, Platform.KUBERNETES)
    try:
        assert gg._health_server is not None  # started by default on k8s
    finally:
        if gg._health_server:
            gg._health_server.stop()


def test_health_explicit_enable_on_host():
    gg = _init_health_for({"enabled": True, "port": 0}, Platform.HOST)
    try:
        assert gg._health_server is not None
    finally:
        if gg._health_server:
            gg._health_server.stop()


def test_health_explicit_disable_on_kubernetes():
    gg = _init_health_for({"enabled": False, "port": 0}, Platform.KUBERNETES)
    try:
        assert gg._health_server is None  # explicit false beats the k8s default
    finally:
        if gg._health_server:
            gg._health_server.stop()


# ---------------------------------------------------------------------------
# MessagingClient.connected() accessor
# ---------------------------------------------------------------------------


class _FakeProvider:
    """Minimal messaging provider double for connected()/disconnect() wiring tests."""

    def __init__(self, connected=True):
        self._connected = connected
        self.subscriptions = {"topic/a": object(), "topic/b": object()}
        self.disconnect_calls = 0

    def connected(self):
        return self._connected

    def disconnect(self):
        # The real providers unsubscribe every tracked subscription on disconnect.
        self.disconnect_calls += 1
        self.subscriptions.clear()


def test_messaging_connected_accessor():
    prev = MessagingClient._messaging_provider
    try:
        MessagingClient._messaging_provider = None
        assert MessagingClient.connected() is False  # no provider -> not connected
        MessagingClient._messaging_provider = _FakeProvider(connected=True)
        assert MessagingClient.connected() is True
        MessagingClient._messaging_provider = _FakeProvider(connected=False)
        assert MessagingClient.connected() is False
    finally:
        MessagingClient._messaging_provider = prev


# ---------------------------------------------------------------------------
# FR-HB-2: shutdown flips readiness to 503 + unsubscribes all (idempotent)
# ---------------------------------------------------------------------------


def _bare_gg_for_shutdown(readiness, health_server=None):
    gg = GGCommons.__new__(GGCommons)
    # All subsystems shutdown() touches, set to None except the ones under test.
    gg._stream_metrics = None
    gg._streams = None
    gg._credential_metrics = None
    gg._credentials = None
    gg._parameters = None
    gg._heartbeat = None
    gg._config_manager = None
    gg._readiness = readiness
    gg._health_server = health_server
    gg._sigterm_installed = False
    gg._prev_sigterm_handler = None
    gg._sigint_installed = False
    gg._prev_sigint_handler = None
    return gg


def test_shutdown_flips_readiness_and_unsubscribes(server_factory):
    flag = _ConnFlag(True)
    readiness = ReadinessState(flag)
    srv = server_factory(readiness)
    # Healthy first.
    assert _get(srv.port, "/readyz")[0] == 200

    fake = _FakeProvider(connected=True)
    prev = MessagingClient._messaging_provider
    MessagingClient._messaging_provider = fake
    try:
        gg = _bare_gg_for_shutdown(readiness, health_server=srv)
        gg.shutdown()

        # Readiness latched to "shutting down" => not ready.
        assert readiness.is_ready() is False
        # All tracked subscriptions torn down via the provider's disconnect().
        assert fake.disconnect_calls == 1
        assert fake.subscriptions == {}
        # MessagingClient.shutdown() nulled the provider.
        assert MessagingClient._messaging_provider is None

        # Idempotent: a second shutdown must not raise.
        gg.shutdown()
    finally:
        MessagingClient._messaging_provider = prev


def test_sigterm_handler_flips_readiness_and_exits(monkeypatch):
    flag = _ConnFlag(True)
    readiness = ReadinessState(flag)
    gg = _bare_gg_for_shutdown(readiness)

    prev = MessagingClient._messaging_provider
    MessagingClient._messaging_provider = _FakeProvider(connected=True)
    try:
        # The handler must end the process with exit code 0.
        with pytest.raises(SystemExit) as exc:
            gg._handle_termination_signal(15, None)
        assert exc.value.code == 0
        assert readiness.is_ready() is False
    finally:
        MessagingClient._messaging_provider = prev


def test_install_wires_both_sigterm_and_sigint_and_restores_both(monkeypatch):
    """#21 FR-HB-2 parity: the library wires BOTH SIGTERM and SIGINT to the graceful-shutdown path,
    and restores the previous handler for each on shutdown (Java's JVM hook fires on SIGTERM+SIGINT,
    TS wires both process.on signals, Rust awaits SIGTERM + Ctrl-C). Before the fix only SIGTERM was
    installed, so an interactive Ctrl-C (SIGINT) bypassed the library-owned shutdown."""
    calls = []
    sentinel_prev = object()

    def fake_signal(signum, handler):
        calls.append((signum, handler))
        return sentinel_prev  # pretend each signal already had a handler

    monkeypatch.setattr(signal, "signal", fake_signal)

    gg = _bare_gg_for_shutdown(readiness=None)
    gg._install_signal_handlers()

    installed = {signum for signum, _ in calls}
    assert signal.SIGTERM in installed
    assert signal.SIGINT in installed
    assert gg._sigterm_installed is True
    assert gg._sigint_installed is True
    assert gg._prev_sigterm_handler is sentinel_prev
    assert gg._prev_sigint_handler is sentinel_prev

    # On shutdown both signals are restored to their previous handlers and the flags reset.
    calls.clear()
    prev = MessagingClient._messaging_provider
    MessagingClient._messaging_provider = _FakeProvider(connected=True)
    try:
        gg.shutdown()
    finally:
        MessagingClient._messaging_provider = prev

    restored = {signum for signum, handler in calls if handler is sentinel_prev}
    assert signal.SIGTERM in restored
    assert signal.SIGINT in restored
    assert gg._sigterm_installed is False
    assert gg._sigint_installed is False
