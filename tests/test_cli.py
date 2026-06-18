"""
Unit tests for the CLI contract enforced by GGCommons argument processing.

These exercise the validation paths that raise before any messaging/broker setup,
so they run without a broker (no integration marker).
"""

import pytest

from ggcommons.ggcommons import GGCommons


def test_unknown_mode_is_rejected():
    # An unrecognized -m mode must be rejected, not silently treated as GREENGRASS.
    with pytest.raises(ValueError, match="Unknown mode"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "-m", "BOGUS"])


def test_standalone_without_path_is_rejected():
    with pytest.raises(ValueError, match="STANDALONE mode requires"):
        GGCommons("com.test.C", ["-c", "FILE", "x.json", "-m", "STANDALONE"])


def test_unknown_config_source_is_rejected():
    with pytest.raises(ValueError, match="Unrecognized config source"):
        GGCommons("com.test.C", ["-c", "BOGUS"])
