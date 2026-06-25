"""Phase-0 remediation regression tests.

Covers the two consumer-side identity bugs the adversarial review caught:
- mustFix #1: the resolved identity must actually be USED by the config builder
  (canonical precedence -t > AWS_IOT_THING_NAME > NOT_GREENGRASS), not re-derived
  with the old env>-t>uuid4 logic.
- mustFix #4: --transport MQTT without a messaging-config path raises (deferred to
  MessagingClient.init), which previously had no test after the parse-time check moved.

See docs/platform/ (DESIGN-core §6.2, §12).
"""
from argparse import Namespace

import pytest

import ggcommons.config.manager.config_manager_builder as builder_mod
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.platform import Transport


def test_mqtt_transport_without_path_raises():
    """--transport MQTT with no messaging-config path -> RuntimeError at init (mustFix #4)."""
    args = Namespace(transport=Transport.MQTT, identity="t", thing=None)
    with pytest.raises(RuntimeError, match="messaging config"):
        MessagingClient.init(args, standalone_config_path=None)


def _thing_name_from_build(monkeypatch, args):
    """Build a config manager with the GreengrassConfigManager stubbed out (no IPC),
    returning the thing_name the builder computed and passed to the manager."""
    captured = {}

    class _StubMgr:
        def __init__(self, thing_name, component_name, *a, **k):
            captured["thing_name"] = thing_name

        def initialize(self, *a, **k):  # builder/init may call this; no-op
            pass

    monkeypatch.setattr(builder_mod, "GreengrassConfigManager", _StubMgr)
    builder_mod.ConfigManagerBuilder.build(args, "com.example.C")
    return captured["thing_name"]


def test_config_builder_uses_resolved_identity(monkeypatch):
    """ConfigManagerBuilder uses the resolver's identity, not a re-derived value (mustFix #1)."""
    monkeypatch.setenv("AWS_IOT_THING_NAME", "env-should-be-ignored")
    args = Namespace(config=["GG_CONFIG"], thing=None, identity="resolved-thing")
    assert _thing_name_from_build(monkeypatch, args) == "resolved-thing"


def test_config_builder_default_is_not_greengrass_not_uuid(monkeypatch):
    """No -t / no env / no resolved identity -> canonical constant NOT_GREENGRASS (not a random uuid4)."""
    monkeypatch.delenv("AWS_IOT_THING_NAME", raising=False)
    args = Namespace(config=["GG_CONFIG"], thing=None)  # no .identity attribute at all
    assert _thing_name_from_build(monkeypatch, args) == "NOT_GREENGRASS"


def test_config_builder_fallback_precedence_thing_over_env(monkeypatch):
    """Fallback (no resolved identity) mirrors canonical precedence: -t beats AWS_IOT_THING_NAME."""
    monkeypatch.setenv("AWS_IOT_THING_NAME", "env-thing")
    args = Namespace(config=["GG_CONFIG"], thing="flag-thing")  # no .identity
    assert _thing_name_from_build(monkeypatch, args) == "flag-thing"
