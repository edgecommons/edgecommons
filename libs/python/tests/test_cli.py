"""
Unit tests for the CLI contract enforced by GGCommons argument processing,
re-pointed from the removed ``-m/--mode`` flag to the two-axis
``--platform``/``--transport`` contract (DESIGN-core sec 6.1).

These exercise the validation paths that raise before any messaging/broker setup,
so they run without a broker (no integration marker).
"""

import pytest

from ggcommons.ggcommons import GGCommons


def test_legacy_mode_flag_is_rejected_with_guidance():
    # The removed -m/--mode flag must be rejected with guidance to the new axes.
    with pytest.raises(ValueError, match="--platform"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "-m", "STANDALONE"])


def test_legacy_long_mode_flag_is_rejected_with_guidance():
    with pytest.raises(ValueError, match="--transport"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "--mode", "GREENGRASS"])


def test_unknown_platform_is_rejected():
    with pytest.raises(ValueError, match="Unknown platform"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "BOGUS"])


def test_unknown_transport_is_rejected():
    with pytest.raises(ValueError, match="Unknown transport"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "HOST", "--transport", "BOGUS"])


def test_kubernetes_platform_fails_fast_in_phase0():
    with pytest.raises(ValueError, match="KUBERNETES"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "KUBERNETES"])


def test_ipc_on_host_fails_the_ipc_lock():
    with pytest.raises(ValueError, match="IPC transport requires --platform GREENGRASS"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "--platform", "HOST", "--transport", "IPC"])


def test_unknown_config_source_is_rejected():
    with pytest.raises(ValueError, match="Unrecognized config source"):
        GGCommons("com.test.C", ["-c", "BOGUS"])
