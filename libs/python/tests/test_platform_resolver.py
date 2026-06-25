"""
Unit tests for the pure platform resolver (DESIGN-core sec 4), the auto-detector
(sec 5), the IPC-lock validation (sec 4.1) and identity resolution (sec 6.2).

Mirrors the canonical Java ``PlatformResolverTest``. Exercised in isolation with
injected environments and a filesystem probe so the suite is the Phase-0 oracle.
"""

import pytest

from ggcommons.platform import (
    DEFAULT_IDENTITY,
    ENV_GG_IPC_SOCKET,
    ENV_GG_SVCUID,
    ENV_K8S_SERVICE_HOST,
    ENV_THING_NAME,
    K8S_SA_TOKEN_PATH,
    PROFILES,
    Platform,
    ResolverInputs,
    Transport,
    detect_platform,
    resolve_identity,
    resolve_profile,
    validate,
)

NO_FILES = lambda p: False
ALL_FILES = lambda p: True


# ---------- detect_platform ----------

def test_detect_greengrass_from_ipc_socket_env():
    env = {ENV_GG_IPC_SOCKET: "/run/gg.sock"}
    assert detect_platform(env, NO_FILES) == Platform.GREENGRASS


def test_detect_greengrass_from_svcuid_env():
    env = {ENV_GG_SVCUID: "abc123"}
    assert detect_platform(env, NO_FILES) == Platform.GREENGRASS


def test_detect_kubernetes_from_token_file():
    only_token = lambda p: p == K8S_SA_TOKEN_PATH
    assert detect_platform({}, only_token) == Platform.KUBERNETES


def test_detect_kubernetes_from_service_host_env():
    env = {ENV_K8S_SERVICE_HOST: "10.0.0.1"}
    assert detect_platform(env, NO_FILES) == Platform.KUBERNETES


def test_detect_host_when_no_signals():
    assert detect_platform({}, NO_FILES) == Platform.HOST


def test_greengrass_wins_over_kubernetes_when_both_present():
    # A containerized Nucleus component can set both; GREENGRASS must win (load-bearing order).
    env = {ENV_GG_SVCUID: "uid", ENV_K8S_SERVICE_HOST: "10.0.0.1"}
    assert detect_platform(env, ALL_FILES) == Platform.GREENGRASS


def test_empty_env_value_is_not_a_signal():
    env = {ENV_GG_SVCUID: ""}
    assert detect_platform(env, NO_FILES) == Platform.HOST


def test_public_detect_uses_real_filesystem_probe():
    # The token path does not exist on the test host -> HOST, with no env signals.
    assert detect_platform({}) == Platform.HOST


# ---------- resolve_profile: profile defaults ----------

def test_resolve_greengrass_explicit_gives_ipc_and_gg_config():
    inputs = ResolverInputs(Platform.GREENGRASS, None, None, None)
    r = resolve_profile(inputs, {})
    assert r.platform == Platform.GREENGRASS
    assert r.transport == Transport.IPC
    assert r.config_source == ["GG_CONFIG"]
    assert r.identity == DEFAULT_IDENTITY


def test_resolve_host_explicit_gives_mqtt_and_gg_config_in_phase0():
    # Phase 0 deliberately keeps HOST's default config source at GG_CONFIG (not FILE).
    inputs = ResolverInputs(Platform.HOST, None, None, None)
    r = resolve_profile(inputs, {})
    assert r.platform == Platform.HOST
    assert r.transport == Transport.MQTT
    assert r.config_source == ["GG_CONFIG"]


def test_resolve_auto_with_no_signals_detects_host():
    inputs = ResolverInputs(None, None, None, None)
    r = resolve_profile(inputs, {})
    assert r.platform == Platform.HOST
    assert r.transport == Transport.MQTT


def test_resolve_auto_with_greengrass_env_detects_greengrass():
    inputs = ResolverInputs(None, None, None, None)
    r = resolve_profile(inputs, {ENV_GG_IPC_SOCKET: "/run/gg.sock"})
    assert r.platform == Platform.GREENGRASS
    assert r.transport == Transport.IPC


# ---------- resolve_profile: explicit overrides ----------

def test_explicit_config_args_override_profile_default():
    inputs = ResolverInputs(Platform.GREENGRASS, None, ["FILE", "/etc/cfg.json"], None)
    r = resolve_profile(inputs, {})
    assert r.config_source == ["FILE", "/etc/cfg.json"]


def test_explicit_transport_overrides_profile_default():
    inputs = ResolverInputs(Platform.HOST, Transport.MQTT, None, None)
    r = resolve_profile(inputs, {})
    assert r.transport == Transport.MQTT


def test_explicit_thing_overrides_env_probe():
    inputs = ResolverInputs(Platform.HOST, None, None, "my-thing")
    r = resolve_profile(inputs, {ENV_THING_NAME: "env-thing"})
    assert r.identity == "my-thing"


# ---------- resolve_profile: failures ----------

def test_resolve_kubernetes_fails_fast_in_phase0():
    inputs = ResolverInputs(Platform.KUBERNETES, None, None, None)
    with pytest.raises(ValueError, match="KUBERNETES"):
        resolve_profile(inputs, {})


def test_resolve_ipc_on_host_fails_the_ipc_lock():
    inputs = ResolverInputs(Platform.HOST, Transport.IPC, None, None)
    with pytest.raises(ValueError, match="IPC transport requires --platform GREENGRASS"):
        resolve_profile(inputs, {})


# ---------- validate ----------

def test_validate_rejects_ipc_on_non_greengrass():
    with pytest.raises(ValueError):
        validate(Platform.HOST, Transport.IPC)
    with pytest.raises(ValueError):
        validate(Platform.KUBERNETES, Transport.IPC)


def test_validate_accepts_legal_combos():
    validate(Platform.GREENGRASS, Transport.IPC)
    validate(Platform.HOST, Transport.MQTT)
    validate(Platform.KUBERNETES, Transport.MQTT)


# ---------- resolve_identity ----------

def test_resolve_identity_prefers_explicit_thing():
    assert resolve_identity("t1", Platform.GREENGRASS, {}) == "t1"


def test_resolve_identity_falls_back_to_env():
    assert resolve_identity(None, Platform.HOST, {ENV_THING_NAME: "env-thing"}) == "env-thing"


def test_resolve_identity_defaults_when_nothing_available():
    assert resolve_identity(None, Platform.HOST, {}) == DEFAULT_IDENTITY


def test_resolve_identity_handles_none_env():
    assert resolve_identity(None, Platform.HOST, None) == DEFAULT_IDENTITY


# ---------- profiles + enums ----------

def test_profiles_contain_only_greengrass_and_host_in_phase0():
    assert len(PROFILES) == 2
    assert Platform.GREENGRASS in PROFILES
    assert Platform.HOST in PROFILES
    assert Platform.KUBERNETES not in PROFILES


def test_enums_declare_expected_values():
    assert len(list(Platform)) == 3
    assert Platform("KUBERNETES") == Platform.KUBERNETES
    assert len(list(Transport)) == 2
    assert Transport("IPC") == Transport.IPC


def test_profile_record_exposes_its_fields():
    p = PROFILES[Platform.GREENGRASS]
    assert p.transport == Transport.IPC
    assert p.config_source == "GG_CONFIG"
