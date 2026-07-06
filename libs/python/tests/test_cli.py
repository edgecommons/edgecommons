"""
Unit tests for the CLI contract enforced by EdgeCommons argument processing,
re-pointed from the removed ``-m/--mode`` flag to the two-axis
``--platform``/``--transport`` contract (DESIGN-core sec 6.1).

These exercise the validation paths that raise before any messaging/broker setup,
so they run without a broker (no integration marker).
"""

import os

import pytest

from edgecommons.edgecommons import EdgeCommons
from edgecommons.config.manager.configmap_config_manager import (
    DEFAULT_KEY,
    DEFAULT_MOUNT_DIR,
)
from edgecommons.platform import Transport


def _parse(args):
    """Drive only EdgeCommons._process_args (arg parse + profile resolution), without running the
    full __init__ (which would spin up messaging/metrics). Returns the parsed namespace."""
    obj = EdgeCommons.__new__(EdgeCommons)
    return obj._process_args("com.test.C", args, None)


# ---------- FR-MSG-1: default messaging-config path from the mounted ConfigMap ----------

def test_configmap_mqtt_defaults_messaging_path_to_mounted_configmap_file():
    # Explicit KUBERNETES + CONFIGMAP (default mount/key) + MQTT, no positional messaging path:
    # the messaging-config path defaults to the resolved ConfigMap file (/etc/edgecommons/config.json).
    parsed = _parse(["--platform", "KUBERNETES", "--transport", "MQTT", "-c", "CONFIGMAP"])
    assert parsed.transport == Transport.MQTT
    assert parsed.config[0].upper() == "CONFIGMAP"
    assert parsed.standalone_config_path == os.path.join(DEFAULT_MOUNT_DIR, DEFAULT_KEY)


def test_configmap_mqtt_default_uses_profile_transport_when_transport_omitted():
    # KUBERNETES profile derives MQTT, and CONFIGMAP is the KUBERNETES default config source, so
    # even with neither --transport nor -c given the messaging path still defaults to the ConfigMap.
    parsed = _parse(["--platform", "KUBERNETES"])
    assert parsed.transport == Transport.MQTT
    assert parsed.config == ["CONFIGMAP"]
    assert parsed.standalone_config_path == os.path.join(DEFAULT_MOUNT_DIR, DEFAULT_KEY)


def test_configmap_mqtt_default_honors_custom_mount_dir_and_key():
    # The default uses the SAME dir/key the CONFIGMAP source resolves from `-c CONFIGMAP [dir] [key]`.
    parsed = _parse(
        ["--platform", "KUBERNETES", "--transport", "MQTT", "-c", "CONFIGMAP", "/custom/mnt", "app.json"]
    )
    assert parsed.standalone_config_path == os.path.join("/custom/mnt", "app.json")


def test_configmap_mqtt_default_honors_custom_mount_dir_only():
    parsed = _parse(["--platform", "KUBERNETES", "-c", "CONFIGMAP", "/custom/mnt"])
    assert parsed.standalone_config_path == os.path.join("/custom/mnt", DEFAULT_KEY)


def test_explicit_messaging_path_is_not_overridden_under_configmap():
    # The existing explicit-path behavior is unchanged: an explicit `--transport MQTT <path>` wins
    # over the CONFIGMAP default.
    parsed = _parse(
        ["--platform", "KUBERNETES", "--transport", "MQTT", "/explicit/messaging.json", "-c", "CONFIGMAP"]
    )
    assert parsed.standalone_config_path == "/explicit/messaging.json"


def test_no_configmap_default_under_file_source():
    # Only the CONFIGMAP source triggers the default — FILE+MQTT still has no messaging path.
    parsed = _parse(["--platform", "HOST", "--transport", "MQTT", "-c", "FILE", "x.json"])
    assert parsed.config[0].upper() == "FILE"
    assert parsed.standalone_config_path is None


def test_host_mqtt_does_not_default_messaging_path():
    # Only CONFIGMAP synthesizes a default messaging path; HOST defaults its config source to FILE
    # (not CONFIGMAP), so HOST+MQTT must NOT get a defaulted messaging path — it still requires an
    # explicit one.
    parsed = _parse(["--platform", "HOST", "--transport", "MQTT"])
    assert parsed.transport == Transport.MQTT
    assert parsed.config == ["FILE"]
    assert parsed.standalone_config_path is None


def test_host_mqtt_without_path_still_raises_at_messaging_init():
    # End-to-end parity: HOST+MQTT with no messaging-config path fails fast at messaging init (the
    # first init step), before any broker/config work — confirming HOST behavior is unchanged.
    with pytest.raises(RuntimeError, match="MQTT transport requires a messaging config file path"):
        EdgeCommons("com.test.C", ["--platform", "HOST", "--transport", "MQTT"])


def test_legacy_mode_flag_is_rejected_with_guidance():
    # The removed -m/--mode flag must be rejected with guidance to the new axes.
    with pytest.raises(ValueError, match="--platform"):
        EdgeCommons("com.test.C", ["-c", "FILE", "x.json", "-m", "STANDALONE"])


def test_legacy_long_mode_flag_is_rejected_with_guidance():
    with pytest.raises(ValueError, match="--transport"):
        EdgeCommons("com.test.C", ["-c", "FILE", "x.json", "--mode", "GREENGRASS"])


def test_unknown_platform_is_rejected():
    with pytest.raises(ValueError, match="Unknown platform"):
        EdgeCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "BOGUS"])


def test_unknown_transport_is_rejected():
    with pytest.raises(ValueError, match="Unknown transport"):
        EdgeCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "HOST", "--transport", "BOGUS"])


def test_ipc_on_kubernetes_fails_the_ipc_lock():
    # Phase 1a: KUBERNETES resolves cleanly (no fail-fast), but the IPC lock still holds — only the
    # Greengrass Nucleus provides the IPC socket. This raises during resolution, before broker setup.
    with pytest.raises(ValueError, match="IPC transport requires --platform GREENGRASS"):
        EdgeCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "KUBERNETES", "--transport", "IPC"])


def test_ipc_on_host_fails_the_ipc_lock():
    with pytest.raises(ValueError, match="IPC transport requires --platform GREENGRASS"):
        EdgeCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "HOST", "--transport", "IPC"])


def test_unknown_config_source_is_rejected():
    with pytest.raises(ValueError, match="Unrecognized config source"):
        EdgeCommons("com.test.C", ["-c", "BOGUS"])
