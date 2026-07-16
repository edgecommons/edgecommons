"""Unit tests for the config subsystem: ConfigManager base, ConfigurationValidator,
EnvironmentConfigManager, FileConfigManager, ConfigManagerBuilder dispatch, and
ConfigComponentManager (with a mocked MessagingClient).

All paths run in-process: temp files, env vars, and mocks stand in for any live source.
"""
import json
import os
from argparse import Namespace
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest

from edgecommons.config.manager.config_manager import ConfigManager, _sanitize
from edgecommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from edgecommons.validation.configuration_validator import (
    ConfigurationValidator,
    ConfigurationValidationException,
)


class DictConfigManager(ConfigManager):
    """ConfigManager whose source is a fixed dict (no live config source)."""

    def __init__(self, cfg, component="com.example.MyComp", thing="thing-1", **kw):
        self._cfg = cfg
        super().__init__(component, thing, **kw)
        self.init()

    def _load_configuration(self):
        return self._cfg


# --------------------------------------------------------------------------- base


class TestConfigManagerBase:
    def test_empty_component_name_raises(self):
        with pytest.raises(ValueError):
            ConfigManager("")

    def test_component_short_vs_full_name(self):
        cm = DictConfigManager({"component": {}})
        assert cm.get_component_name() == "MyComp"
        assert cm.get_component_full_name() == "com.example.MyComp"
        assert cm.get_thing_name() == "thing-1"

    def test_accessors_populated_after_init(self):
        cm = DictConfigManager({"component": {"global": {"k": "v"}, "instances": [{"id": "main"}]}})
        assert cm.get_metric_config() is not None
        assert cm.get_heartbeat_config() is not None
        assert cm.get_health_config() is not None
        assert cm.get_logging_config() is not None
        assert cm.get_tag_config() is not None
        assert cm.get_global_config() == {"k": "v"}
        assert cm.get_instance_ids() == ["main"]
        assert cm.get_instance_config("main") == {"id": "main"}
        assert cm.get_config_source() == "unknown"
        assert cm.is_validation_enabled() is True

    def test_initializing_then_complete(self):
        cm = DictConfigManager({"component": {}})
        assert cm.is_initializing() is True
        cm.complete_initialization()
        assert cm.is_initializing() is False

    def test_get_full_config_includes_optional_sections(self):
        # validation disabled: this test only checks get_full_config surfaces the
        # accepted effective snapshot exactly.
        cm = DictConfigManager({
            "component": {"global": {}, "instances": []},
            "streaming": {"streams": []},
            "credentials": {"vault": {}},
            "parameters": {"sources": []},
        }, validate_config=False)
        full = cm.get_full_config()
        assert full["streaming"] == {"streams": []}
        assert full["credentials"] == {"vault": {}}
        assert full["parameters"] == {"sources": []}
        assert full["component"] == {"global": {}, "instances": []}
        assert "metricEmission" not in full

    def test_logging_correlation_reads_k8s_env(self, monkeypatch):
        monkeypatch.setenv("POD_NAME", "pod-1")
        monkeypatch.setenv("POD_NAMESPACE", "ns-1")
        monkeypatch.setenv("NODE_NAME", "node-1")
        cm = DictConfigManager({"component": {}})
        corr = cm._logging_correlation()
        assert corr["thing"] == "thing-1"
        assert corr["pod"] == "pod-1"
        assert corr["namespace"] == "ns-1"
        assert corr["node"] == "node-1"

    def test_listeners_add_notify_remove(self):
        cm = DictConfigManager({"component": {}})
        cm.complete_initialization()
        events = []

        class L(ConfigurationChangeListener):
            def on_configuration_change(self, cfg):
                events.append(cfg)
                return True

        listener = L()
        cm.add_config_change_listener(listener)
        assert cm.configuration_changed({"component": {}}) is True
        assert len(events) == 1
        cm.remove_config_change_listener(listener)
        cm.configuration_changed({"component": {}})
        assert len(events) == 1  # no longer notified

    def test_add_remove_none_listener_raises(self):
        cm = DictConfigManager({"component": {}})
        with pytest.raises(ValueError):
            cm.add_config_change_listener(None)
        with pytest.raises(ValueError):
            cm.remove_config_change_listener(None)

    def test_remove_unknown_listener_warns_not_raises(self):
        cm = DictConfigManager({"component": {}})

        class L(ConfigurationChangeListener):
            def on_configuration_change(self, cfg):
                return True

        cm.remove_config_change_listener(L())  # not present -> warning, no raise

    def test_listener_returning_false_is_logged(self):
        cm = DictConfigManager({"component": {}})
        cm.complete_initialization()

        class L(ConfigurationChangeListener):
            def on_configuration_change(self, cfg):
                return False

        cm.add_config_change_listener(L())
        # still returns True overall (apply succeeded); listener False just logs
        assert cm.configuration_changed({"component": {}}) is True

    def test_listener_exception_is_swallowed(self):
        cm = DictConfigManager({"component": {}})
        cm.complete_initialization()

        class L(ConfigurationChangeListener):
            def on_configuration_change(self, cfg):
                raise RuntimeError("boom")

        cm.add_config_change_listener(L())
        assert cm.configuration_changed({"component": {}}) is True

    def test_configuration_changed_apply_failure_returns_false(self):
        cm = DictConfigManager({"component": {}})
        cm.complete_initialization()
        # component as a non-dict makes _apply_config raise -> returns False
        assert cm.configuration_changed({"component": "not-a-dict"}) is False

    def test_init_raises_on_invalid_config_while_initializing(self):
        # extra top-level property -> schema rejects (top level is strict)
        with pytest.raises(Exception):
            DictConfigManager({"component": {}, "bogusTopLevelKey": 1})

    def test_invalid_hot_reload_is_rejected_and_prior_config_kept(self):
        """#20 parity: a schema-invalid HOT RELOAD is reject-and-keep — the prior config is retained,
        listeners are NOT notified, and configuration_changed returns False (matching Java/Rust/TS).
        Before the fix the validation failure was swallowed on the reload path and the invalid config
        was applied + broadcast."""
        cm = DictConfigManager({"component": {"global": {"k": "v1"}}}, validate_config=True)
        cm.complete_initialization()  # _initializing == False -> the hot-reload path

        notified = []

        class _Listener(ConfigurationChangeListener):
            def on_configuration_change(self, cfg):
                notified.append(cfg)
                return True

        cm.add_config_change_listener(_Listener())

        # extra top-level property -> schema-invalid; top level is strict (additionalProperties:false)
        invalid = {"component": {"global": {"k": "v2"}}, "bogusTopLevelKey": 1}
        assert cm.configuration_changed(invalid) is False  # rejected
        assert notified == []                              # listeners NOT notified
        assert cm.get_global_config() == {"k": "v1"}       # last-good config retained

    def test_close_is_noop_by_default(self):
        cm = DictConfigManager({"component": {}})
        cm.close()  # no error

    def test_validate_disabled_skips(self):
        # invalid config but validation disabled -> constructs fine
        cm = DictConfigManager({"component": {}, "bogusTopLevelKey": 1}, validate_config=False)
        assert cm.is_validation_enabled() is False


class TestReloadFromProvider:
    """``reload_from_provider`` (DESIGN-uns §9.5 ``reload-config`` command verb
    action): re-fetches via ``_load_configuration`` and re-applies via
    ``configuration_changed``. Also the fullConfig-staleness regression guard: a
    successful reload must be immediately visible on ``get_effective_config()`` (the
    source the effective-config publisher / ``get-configuration`` verb read), not the
    startup snapshot."""

    def test_success_refetches_and_applies(self):
        cm = DictConfigManager({"component": {"global": {"v": 1}}})
        cm.complete_initialization()
        cm._cfg = {"component": {"global": {"v": 2}}}  # what the "source" now holds
        assert cm.reload_from_provider() is True
        assert cm.get_global_config() == {"v": 2}

    def test_success_refreshes_the_effective_config_snapshot_immediately(self):
        """The fullConfig-staleness bug Java's fix addressed: get_effective_config()
        (the cfg publisher / get-configuration verb's source) must reflect the
        reloaded document right after reload_from_provider() returns, not the
        startup snapshot forever."""
        cm = DictConfigManager({"component": {"global": {"v": 1}}})
        cm.complete_initialization()
        assert cm.get_effective_config() == {"component": {"global": {"v": 1}}}
        cm._cfg = {"component": {"global": {"v": 2}}}
        assert cm.reload_from_provider() is True
        assert cm.get_effective_config() == {"component": {"global": {"v": 2}}}

    def test_fetch_exception_returns_false_and_keeps_previous(self):
        cm = DictConfigManager({"component": {"global": {"v": 1}}})
        cm.complete_initialization()

        def boom():
            raise RuntimeError("source unreachable")

        cm._load_configuration = boom
        assert cm.reload_from_provider() is False
        assert cm.get_global_config() == {"v": 1}
        assert cm.get_effective_config() == {"component": {"global": {"v": 1}}}

    def test_none_result_returns_false_and_keeps_previous(self):
        cm = DictConfigManager({"component": {"global": {"v": 1}}})
        cm.complete_initialization()
        cm._load_configuration = lambda: None
        assert cm.reload_from_provider() is False
        assert cm.get_global_config() == {"v": 1}

    def test_schema_invalid_document_returns_false_and_keeps_previous(self):
        cm = DictConfigManager({"component": {"global": {"v": 1}}}, validate_config=True)
        cm.complete_initialization()
        cm._cfg = {"component": {"global": {"v": 2}}, "bogusTopLevelKey": 1}
        assert cm.reload_from_provider() is False
        assert cm.get_global_config() == {"v": 1}
        assert cm.get_effective_config() == {"component": {"global": {"v": 1}}}

    def test_success_notifies_listeners(self):
        cm = DictConfigManager({"component": {"global": {"v": 1}}})
        cm.complete_initialization()
        notified = []

        class L(ConfigurationChangeListener):
            def on_configuration_change(self, cfg):
                notified.append(cfg)
                return True

        cm.add_config_change_listener(L())
        cm._cfg = {"component": {"global": {"v": 2}}}
        assert cm.reload_from_provider() is True
        assert len(notified) == 1


class TestSanitize:
    def test_sanitize_none_is_empty(self):
        assert _sanitize(None) == ""

    def test_sanitize_replaces_hostile_chars(self):
        assert _sanitize("a/b\\c+d#e") == "a_b_c_d_e"

    def test_sanitize_collapses_traversal(self):
        assert _sanitize("..") == "_"


# --------------------------------------------------------------------- validator


class TestConfigurationValidator:
    def test_validate_none_raises_value_error(self):
        with pytest.raises(ValueError):
            ConfigurationValidator.validate(None)

    def test_validate_valid_config_passes(self):
        ConfigurationValidator.validate({"component": {"global": {}, "instances": []}})

    def test_validate_invalid_raises_with_errors(self):
        with pytest.raises(ConfigurationValidationException) as exc:
            ConfigurationValidator.validate({"component": {}, "unexpectedKey": 1})
        assert exc.value.validation_errors  # populated detail list

    def test_validate_section_none_raises(self):
        with pytest.raises(ValueError):
            ConfigurationValidator.validate_section(None, "component")

    def test_validate_section_empty_name_raises(self):
        with pytest.raises(ValueError):
            ConfigurationValidator.validate_section({}, "")

    def test_validate_section_valid(self):
        ConfigurationValidator.validate_section({"global": {}, "instances": []}, "component")

    def test_validate_section_invalid_reraises_with_context(self):
        with pytest.raises(ConfigurationValidationException) as exc:
            # 'tags' must be an object; a string is invalid
            ConfigurationValidator.validate_section("nope", "tags")
        assert "section 'tags'" in str(exc.value)

    def test_is_validation_available(self):
        assert ConfigurationValidator.is_validation_available() is True

    def test_exception_stores_errors(self):
        e = ConfigurationValidationException("msg", [{"a": 1}])
        assert e.validation_errors == [{"a": 1}]
        e2 = ConfigurationValidationException("msg")
        assert e2.validation_errors == []


# ----------------------------------------------------------- environment manager


class TestEnvironmentConfigManager:
    def test_loads_from_env(self, monkeypatch):
        from edgecommons.config.manager.environment_config_manager import EnvironmentConfigManager

        monkeypatch.setenv("MY_CFG", json.dumps({"component": {"global": {"x": 1}}}))
        cm = EnvironmentConfigManager("thing-1", "com.example.C", "MY_CFG")
        assert cm.get_global_config() == {"x": 1}
        assert "MY_CFG" in cm.get_config_source()

    def test_missing_env_raises(self, monkeypatch):
        from edgecommons.config.manager.environment_config_manager import EnvironmentConfigManager

        monkeypatch.delenv("ABSENT_CFG", raising=False)
        with pytest.raises(RuntimeError, match="ABSENT_CFG"):
            EnvironmentConfigManager("thing-1", "com.example.C", "ABSENT_CFG")


# ------------------------------------------------------------------ file manager


class TestFileConfigManager:
    def test_loads_and_watches_then_closes(self, tmp_path):
        from edgecommons.config.manager.file_config_manager import FileConfigManager

        cfg = tmp_path / "config.json"
        cfg.write_text(json.dumps({"component": {"global": {"k": "v"}}}))
        cm = FileConfigManager("thing-1", "com.example.C", str(cfg))
        try:
            assert cm.get_global_config() == {"k": "v"}
            assert "config.json" in cm.get_config_source()
        finally:
            cm.close()
            # close is idempotent
            cm.close()

    def test_missing_file_raises_runtime_error(self, tmp_path):
        from edgecommons.config.manager.file_config_manager import FileConfigManager

        with pytest.raises(RuntimeError, match="Unable to open config file"):
            FileConfigManager("thing-1", "com.example.C", str(tmp_path / "nope.json"))

    def test_change_event_handler_dispatch(self, tmp_path):
        from edgecommons.config.manager.file_config_manager import (
            FileConfigManager,
            ConfigFileChangeEventHandler,
        )

        cfg = tmp_path / "config.json"
        cfg.write_text(json.dumps({"component": {"global": {"k": 1}}}))
        cm = FileConfigManager("thing-1", "com.example.C", str(cfg))
        try:
            handler = ConfigFileChangeEventHandler(cm, str(cfg))
            # directory events are ignored
            handler.on_modified(SimpleNamespace(is_directory=True, src_path=str(cfg)))
            handler.on_created(SimpleNamespace(is_directory=True, src_path=str(cfg)))
            handler.on_moved(SimpleNamespace(is_directory=True, dest_path=str(cfg)))
            # non-matching path ignored
            handler.on_modified(SimpleNamespace(is_directory=False, src_path="other.json"))
            # matching modify -> reload picks up new content
            cfg.write_text(json.dumps({"component": {"global": {"k": 2}}}))
            handler.on_modified(SimpleNamespace(is_directory=False, src_path=str(cfg)))
            assert cm.get_global_config() == {"k": 2}
            # matching create + move also reload
            handler.on_created(SimpleNamespace(is_directory=False, src_path=str(cfg)))
            handler.on_moved(SimpleNamespace(is_directory=False, dest_path=str(cfg)))
        finally:
            cm.close()

    def test_reload_swallows_parse_error(self, tmp_path):
        from edgecommons.config.manager.file_config_manager import (
            FileConfigManager,
            ConfigFileChangeEventHandler,
        )

        cfg = tmp_path / "config.json"
        cfg.write_text(json.dumps({"component": {}}))
        cm = FileConfigManager("thing-1", "com.example.C", str(cfg))
        try:
            handler = ConfigFileChangeEventHandler(cm, str(cfg))
            cfg.write_text("{ broken json")
            # must not raise even though reload fails to parse
            handler.on_modified(SimpleNamespace(is_directory=False, src_path=str(cfg)))
        finally:
            cm.close()


# ---------------------------------------------------------------- builder dispatch


class TestConfigManagerBuilderDispatch:
    def _patch_all(self, monkeypatch):
        import edgecommons.config.manager.config_manager_builder as cmb

        calls = {}

        def make(name):
            def fake(*args, **kwargs):
                calls[name] = (args, kwargs)
                return f"{name}-instance"
            return fake

        for cls_name in (
            "FileConfigManager",
            "ConfigMapConfigManager",
            "EnvironmentConfigManager",
            "GreengrassConfigManager",
            "ShadowConfigManager",
            "ConfigComponentManager",
        ):
            monkeypatch.setattr(cmb, cls_name, make(cls_name))
        return cmb, calls

    def _args(self, config):
        return Namespace(config=config, identity="thing-1", thing="thing-1", platform=None)

    def test_dispatch_file(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        result = cmb.ConfigManagerBuilder.build(self._args(["FILE", "cfg.json"]), "com.example.C")
        assert result == "FileConfigManager-instance"
        assert "FileConfigManager" in calls

    def test_dispatch_configmap(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        cmb.ConfigManagerBuilder.build(self._args(["CONFIGMAP", "/etc/x", "key"]), "com.example.C")
        assert "ConfigMapConfigManager" in calls

    def test_dispatch_env(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        cmb.ConfigManagerBuilder.build(self._args(["ENV", "MYVAR"]), "com.example.C")
        assert "EnvironmentConfigManager" in calls

    def test_dispatch_gg_config(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        cmb.ConfigManagerBuilder.build(self._args(["GG_CONFIG"]), "com.example.C")
        assert "GreengrassConfigManager" in calls

    def test_dispatch_shadow(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        cmb.ConfigManagerBuilder.build(self._args(["SHADOW"]), "com.example.C")
        assert "ShadowConfigManager" in calls

    def test_dispatch_config_component(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        cmb.ConfigManagerBuilder.build(self._args(["CONFIG_COMPONENT"]), "com.example.C")
        assert "ConfigComponentManager" in calls

    def test_dispatch_unrecognized_raises(self, monkeypatch):
        cmb, _ = self._patch_all(monkeypatch)
        with pytest.raises(ValueError, match="Unrecognized config source"):
            cmb.ConfigManagerBuilder.build(self._args(["BOGUS"]), "com.example.C")

    def test_dispatch_resolves_identity_when_absent(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        args = Namespace(config=["FILE", "c.json"], thing="raw-thing", platform=None)
        cmb.ConfigManagerBuilder.build(args, "com.example.C")
        # thing_name resolved from -t flag passed positionally as first arg
        passed_args = calls["FileConfigManager"][0]
        assert passed_args[0] == "raw-thing"

    def test_dispatch_forwards_precommit_lifecycle_before_construction(self, monkeypatch):
        cmb, calls = self._patch_all(monkeypatch)
        validator = lambda candidate, current, phase: None
        cmb.ConfigManagerBuilder.build(
            self._args(["FILE", "c.json"]),
            "com.example.C",
            candidate_validators={"camera": validator},
            validation_timeout_secs=4.0,
        )
        kwargs = calls["FileConfigManager"][1]
        assert kwargs["candidate_validators"] == {"camera": validator}
        assert kwargs["validation_timeout_secs"] == 4.0


# ----------------------------------------------------------- config component mgr


def _lineage_bundle(component, component_config=None):
    if component_config is None:
        component_config = {"component": {}}
    return {
        "lineageVersion": 1,
        "catalogVersion": "unit-test-catalog",
        "component": component,
        "layers": [
            {
                "id": "line/line-7",
                "kind": "scope",
                "scope": {"line": "line-7"},
                "config": {
                    "hierarchy": {"levels": ["line", "device"]},
                    "identity": {"line": "line-7"},
                },
            },
            {
                "id": f"component/{component}",
                "kind": "component",
                "component": component,
                "config": component_config,
            },
        ],
    }


class TestConfigComponentManager:
    def test_init_requests_and_subscribes(self, monkeypatch):
        import edgecommons.config.manager.config_component_manager as ccm
        from edgecommons.messaging.message import Message

        subscribed = {}
        monkeypatch.setattr(
            ccm.MessagingClient, "subscribe_acknowledged",
            staticmethod(
                lambda topic, cb, **kwargs: subscribed.update(topic=topic, cb=cb)
            ),
        )

        reply = Message()
        reply.body = _lineage_bundle("C", {"component": {"global": {"k": "from-component"}}})

        requested = {}

        def fake_request(topic, msg):
            requested.update(topic=topic, msg=msg)
            return SimpleNamespace(get=lambda timeout=None: (True, reply))

        monkeypatch.setattr(ccm.MessagingClient, "request", staticmethod(fake_request))

        mgr = ccm.ConfigComponentManager("thing-1", "com.example.C")
        assert mgr.get_global_config() == {"k": "from-component"}
        # UNS Flow A (D-U19): the GET rides the config server's rendezvous; the pushed
        # set-config lands on this component's OWN inbox.
        assert requested["topic"] == "ecv1/thing-1/config/cmd/get-configuration"
        assert subscribed["topic"] == "ecv1/thing-1/C/cmd/set-config"
        # The bootstrap request self-identifies in the body and carries no identity.
        assert requested["msg"].get_body() == {"component": "C"}
        assert requested["msg"].get_identity() is None

    def test_load_with_str_body(self, monkeypatch):
        import edgecommons.config.manager.config_component_manager as ccm
        from edgecommons.messaging.message import Message

        monkeypatch.setattr(
            ccm.MessagingClient,
            "subscribe_acknowledged",
            staticmethod(lambda topic, cb, **kwargs: None),
        )
        reply = Message()
        reply.body = json.dumps(_lineage_bundle("C", {"component": {"global": {"k": "str-body"}}}))
        monkeypatch.setattr(
            ccm.MessagingClient, "request",
            staticmethod(lambda topic, msg: SimpleNamespace(get=lambda timeout=None: (True, reply))),
        )
        mgr = ccm.ConfigComponentManager("thing-1", "com.example.C")
        assert mgr.get_global_config() == {"k": "str-body"}

    def test_load_and_apply_config_triggers_change(self, monkeypatch):
        import edgecommons.config.manager.config_component_manager as ccm
        from edgecommons.messaging.message import Message

        monkeypatch.setattr(
            ccm.MessagingClient,
            "subscribe_acknowledged",
            staticmethod(lambda topic, cb, **kwargs: None),
        )
        reply = Message()
        reply.body = _lineage_bundle("C")
        monkeypatch.setattr(
            ccm.MessagingClient, "request",
            staticmethod(lambda topic, msg: SimpleNamespace(get=lambda timeout=None: (True, reply))),
        )
        mgr = ccm.ConfigComponentManager("thing-1", "com.example.C")
        mgr.complete_initialization()

        update = Message()
        update.body = _lineage_bundle("C", {"component": {"global": {"k": "updated"}}})
        mgr.load_and_apply_config("topic", update)
        assert mgr.get_global_config() == {"k": "updated"}
