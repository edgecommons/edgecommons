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
    ENV_K8S_POD_NAME,
    ENV_K8S_SERVICE_HOST,
    ENV_K8S_THING_NAME,
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

def test_resolve_kubernetes_explicit_gives_mqtt_and_configmap():
    # Phase 1a: KUBERNETES now resolves cleanly (no fail-fast) to MQTT + the CONFIGMAP source.
    inputs = ResolverInputs(Platform.KUBERNETES, None, None, None)
    r = resolve_profile(inputs, {})
    assert r.platform == Platform.KUBERNETES
    assert r.transport == Transport.MQTT
    assert r.config_source == ["CONFIGMAP"]


def test_resolve_auto_with_service_account_token_detects_kubernetes():
    # A SA-token pod auto-detects to KUBERNETES and gets MQTT + CONFIGMAP.
    inputs = ResolverInputs(None, None, None, None)
    r = resolve_profile(inputs, {ENV_K8S_SERVICE_HOST: "10.0.0.1"})
    assert r.platform == Platform.KUBERNETES
    assert r.transport == Transport.MQTT
    assert r.config_source == ["CONFIGMAP"]


def test_resolve_ipc_on_kubernetes_fails_the_ipc_lock():
    # The IPC lock still holds on KUBERNETES (only the Nucleus provides the IPC socket).
    inputs = ResolverInputs(Platform.KUBERNETES, Transport.IPC, None, None)
    with pytest.raises(ValueError, match="IPC transport requires --platform GREENGRASS"):
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


# ---------- resolve_identity: KUBERNETES Downward-API tier (FR-RT-7 / FR-CFG-6) ----------

def test_k8s_identity_from_ggcommons_thing_name_env():
    # On KUBERNETES, GGCOMMONS_THING_NAME (the mapped pod annotation) is the identity.
    env = {ENV_K8S_THING_NAME: "annotated-thing"}
    assert resolve_identity(None, Platform.KUBERNETES, env) == "annotated-thing"


def test_k8s_identity_from_pod_name_env_when_thing_name_absent():
    # Falls through to POD_NAME (Downward metadata.name via fieldRef) when GGCOMMONS_THING_NAME unset.
    env = {ENV_K8S_POD_NAME: "my-pod-abc123"}
    assert resolve_identity(None, Platform.KUBERNETES, env) == "my-pod-abc123"


def test_k8s_thing_name_takes_precedence_over_pod_name():
    env = {ENV_K8S_THING_NAME: "annotated-thing", ENV_K8S_POD_NAME: "my-pod-abc123"}
    assert resolve_identity(None, Platform.KUBERNETES, env) == "annotated-thing"


def test_k8s_env_tier_takes_precedence_over_aws_iot_thing_name():
    # On KUBERNETES the Downward-API tier wins over the generic AWS_IOT_THING_NAME probe.
    env = {ENV_K8S_THING_NAME: "k8s-thing", ENV_THING_NAME: "aws-thing"}
    assert resolve_identity(None, Platform.KUBERNETES, env) == "k8s-thing"
    env2 = {ENV_K8S_POD_NAME: "k8s-pod", ENV_THING_NAME: "aws-thing"}
    assert resolve_identity(None, Platform.KUBERNETES, env2) == "k8s-pod"


def test_k8s_falls_back_to_aws_iot_thing_name_then_default():
    # With no Downward-API vars, KUBERNETES still honors AWS_IOT_THING_NAME, then the library default.
    assert resolve_identity(None, Platform.KUBERNETES, {ENV_THING_NAME: "aws-thing"}) == "aws-thing"
    assert resolve_identity(None, Platform.KUBERNETES, {}) == DEFAULT_IDENTITY


def test_k8s_explicit_thing_overrides_downward_api_env():
    # -t/--thing is highest precedence on every platform, including KUBERNETES.
    env = {ENV_K8S_THING_NAME: "k8s-thing", ENV_K8S_POD_NAME: "k8s-pod"}
    assert resolve_identity("cli-thing", Platform.KUBERNETES, env) == "cli-thing"


def test_k8s_empty_downward_env_value_falls_through():
    # An empty Downward-API value (unset fieldRef) is treated as absent.
    env = {ENV_K8S_THING_NAME: "", ENV_K8S_POD_NAME: "my-pod"}
    assert resolve_identity(None, Platform.KUBERNETES, env) == "my-pod"
    env2 = {ENV_K8S_THING_NAME: "", ENV_K8S_POD_NAME: "", ENV_THING_NAME: "aws-thing"}
    assert resolve_identity(None, Platform.KUBERNETES, env2) == "aws-thing"


def test_downward_api_env_ignored_on_non_kubernetes_platforms():
    # The KUBERNETES tier is gated on platform: HOST/GREENGRASS ignore GGCOMMONS_THING_NAME/POD_NAME.
    env = {ENV_K8S_THING_NAME: "k8s-thing", ENV_K8S_POD_NAME: "k8s-pod", ENV_THING_NAME: "aws-thing"}
    assert resolve_identity(None, Platform.HOST, env) == "aws-thing"
    assert resolve_identity(None, Platform.GREENGRASS, env) == "aws-thing"
    # And with only the Downward-API vars set, non-k8s platforms fall straight to the default.
    only_k8s = {ENV_K8S_THING_NAME: "k8s-thing", ENV_K8S_POD_NAME: "k8s-pod"}
    assert resolve_identity(None, Platform.HOST, only_k8s) == DEFAULT_IDENTITY


def test_k8s_identity_handles_none_env():
    assert resolve_identity(None, Platform.KUBERNETES, None) == DEFAULT_IDENTITY


def test_resolve_profile_wires_kubernetes_downward_identity():
    # End-to-end: resolve_profile passes the resolved platform into resolve_identity, so a
    # KUBERNETES pod with GGCOMMONS_THING_NAME gets that identity (FR-RT-7 integration).
    inputs = ResolverInputs(Platform.KUBERNETES, None, None, None)
    r = resolve_profile(inputs, {ENV_K8S_THING_NAME: "pod-thing", ENV_THING_NAME: "aws-thing"})
    assert r.identity == "pod-thing"


def test_resolve_profile_kubernetes_pod_name_when_autodetected():
    # Auto-detected KUBERNETES (SA service host) + POD_NAME yields the pod-name identity.
    inputs = ResolverInputs(None, None, None, None)
    env = {ENV_K8S_SERVICE_HOST: "10.0.0.1", ENV_K8S_POD_NAME: "auto-pod"}
    r = resolve_profile(inputs, env)
    assert r.platform == Platform.KUBERNETES
    assert r.identity == "auto-pod"


def test_resolved_kubernetes_identity_passes_template_sanitization():
    # FR-RT-7: the resolver returns the raw value; the existing template-variable sanitization
    # still applies when it is interpolated as {ThingName} (path separators / wildcards / traversal
    # are neutralized). A pod name with hostile characters must come out sanitized.
    from ggcommons.config.manager.config_manager import ConfigManager, _sanitize

    identity = resolve_identity(None, Platform.KUBERNETES, {ENV_K8S_POD_NAME: "ns/../pod+name#x"})
    assert identity == "ns/../pod+name#x"  # resolver does not mutate the raw value

    cm = ConfigManager("com.test.C", identity)
    resolved = cm.resolve_template("buf/{ThingName}/data")
    # The interpolated identity is sanitized; the surrounding template separators are preserved.
    assert resolved == f"buf/{_sanitize(identity)}/data"
    middle = _sanitize(identity)
    for bad in ("/", "\\", "+", "#", ".."):
        assert bad not in middle


# ---------- profiles + enums ----------

def test_profiles_contain_all_three_platforms():
    assert len(PROFILES) == 3
    assert Platform.GREENGRASS in PROFILES
    assert Platform.HOST in PROFILES
    assert Platform.KUBERNETES in PROFILES


def test_kubernetes_profile_exposes_mqtt_and_configmap():
    p = PROFILES[Platform.KUBERNETES]
    assert p.transport == Transport.MQTT
    assert p.config_source == "CONFIGMAP"


def test_enums_declare_expected_values():
    assert len(list(Platform)) == 3
    assert Platform("KUBERNETES") == Platform.KUBERNETES
    assert len(list(Transport)) == 2
    assert Transport("IPC") == Transport.IPC


def test_profile_record_exposes_its_fields():
    p = PROFILES[Platform.GREENGRASS]
    assert p.transport == Transport.IPC
    assert p.config_source == "GG_CONFIG"
