import json
from argparse import Namespace
from pathlib import Path
from types import SimpleNamespace

import pytest

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.configmap_config_manager import ConfigMapConfigManager
from edgecommons.config.manager.file_config_manager import FileConfigManager
from edgecommons.config.manager.split_config import (
    DEFAULT_SHARED_COMPONENT,
    SHARED_COMPONENT_KEY,
    SHARED_SHADOW_NAME,
    SplitConfigError,
    deep_merge_layers,
    parse_config_component_payload,
    derive_catalog_version,
    resolve_configmap_base,
    resolve_env_base,
    resolve_file_base,
    resolve_greengrass_base,
    resolve_shadow_base,
    shared_config_enabled,
    validate_base_layer,
    validate_component_layer,
)


VECTOR_DIR = Path(__file__).resolve().parents[3] / "split-config-test-vectors"


def _vectors(name):
    return json.loads((VECTOR_DIR / name).read_text())["cases"]


class DictLayeredManager(ConfigManager):
    def __init__(self, component_layer, base_layer=None, **kwargs):
        self._component_layer = component_layer
        self._base_layer = base_layer
        super().__init__(
            "com.example.C",
            "thing-1",
            validate_config=kwargs.pop("validate_config", False),
            no_shared_config=kwargs.pop("no_shared_config", False),
            **kwargs,
        )
        self._config_provider_family = "TEST"
        self.init()

    def _load_configuration(self):
        return self._component_layer

    def _resolve_base_layer(self, component_layer):
        from edgecommons.config.manager.split_config import BaseLayer

        return BaseLayer(self._base_layer, "test-base" if self._base_layer is not None else None)


class ConfigComponentLayeredManager(ConfigManager):
    def __init__(self, source_payload, **kwargs):
        self._source_payload = source_payload
        super().__init__(
            "com.example.C",
            "thing-1",
            validate_config=kwargs.pop("validate_config", False),
            **kwargs,
        )
        self._config_provider_family = "CONFIG_COMPONENT"
        self._config_source = "CONFIG_COMPONENT"
        self.init()

    def _load_configuration(self):
        return self._source_payload


def test_merge_vectors():
    for case in _vectors("merge.json"):
        expected = case["expected"]
        base = case["input"].get("base")
        component = case["input"].get("component", {})
        no_shared = case["input"].get("options", {}).get("noSharedConfig", False)

        if expected.get("error") == "N_LAYER_INHERITANCE_NOT_IMPLEMENTED":
            with pytest.raises(SplitConfigError) as exc:
                validate_base_layer(base)
            assert exc.value.code == expected["error"]
            continue

        layers = [component]
        if base is not None and shared_config_enabled(component, no_shared):
            layers = [base, component]
        warnings = []
        effective = deep_merge_layers(
            layers,
            lambda path, left, right: warnings.append(
                {"path": path, "code": "TYPE_CONFLICT_COMPONENT_WINS"}
            ),
        )
        assert effective == expected["effective"], case["name"]
        if "warnings" in expected:
            assert warnings == expected["warnings"]


def test_file_and_configmap_resolution_vectors(tmp_path, monkeypatch):
    cases = {case["name"]: case for case in _vectors("resolution.json")}
    shared = tmp_path / "shared.json"
    shared.write_text(json.dumps({"logging": {"level": "INFO"}}))
    component = tmp_path / "config.json"
    component.write_text("{}")

    base = resolve_file_base(
        str(component),
        cases["file-extends-relative"]["input"]["componentLayer"],
        env={},
    )
    assert base.value == {"logging": {"level": "INFO"}}
    assert base.source == str(shared.resolve())

    env_base = tmp_path / "base.json"
    env_base.write_text(json.dumps({"tags": {"site": "dallas"}}))
    base = resolve_file_base(
        str(component),
        {},
        env={"EDGECOMMONS_SHARED_CONFIG": str(env_base)},
    )
    assert base.value == {"tags": {"site": "dallas"}}

    monkeypatch.setattr(
        "edgecommons.config.manager.split_config.DEFAULT_FILE_SHARED_CONFIG",
        str(tmp_path / "missing-default.json"),
    )
    assert resolve_file_base(str(component), {}, env={}).value is None

    cm_mount = tmp_path / "cm"
    cm_mount.mkdir()
    (cm_mount / "shared.json").write_text(json.dumps({"heartbeat": {"enabled": True}}))
    (cm_mount / "config.json").write_text("{}")
    base = resolve_configmap_base(str(cm_mount), str(cm_mount / "config.json"), {}, env={})
    assert base.value == {"heartbeat": {"enabled": True}}
    assert base.source == str((cm_mount / "shared.json").resolve())


def test_env_gg_config_and_shadow_resolution_vectors(tmp_path):
    cases = {case["name"]: case for case in _vectors("resolution.json")}

    inline = resolve_env_base(cases["env-inline-json"]["input"]["env"])
    assert inline.value == cases["env-inline-json"]["expected"]["base"]

    path = tmp_path / "shared.json"
    path.write_text(json.dumps({"logging": {"level": "WARN"}}))
    at_path = resolve_env_base({"EDGECOMMONS_SHARED_CONFIG": f"@{path}"})
    assert at_path.value == {"logging": {"level": "WARN"}}

    class MissingDefaultClient:
        def get_configuration(self, component_name=None):
            assert component_name == DEFAULT_SHARED_COMPONENT
            return SimpleNamespace(value={})

        def close(self):
            pass

    assert resolve_greengrass_base(env={}, client_factory=MissingDefaultClient).value is None

    class MissingExplicitClient:
        def get_configuration(self, component_name=None):
            assert component_name == "com.example.SharedConfig"
            return SimpleNamespace(value={})

        def close(self):
            pass

    with pytest.raises(SplitConfigError) as exc:
        resolve_greengrass_base(
            env=cases["gg-config-explicit-env-missing-fails"]["input"]["env"],
            client_factory=MissingExplicitClient,
        )
    assert exc.value.code == "SHARED_CONFIG_UNAVAILABLE"

    class ShadowMissingClient:
        def get_thing_shadow(self, thing_name=None, shadow_name=None):
            assert shadow_name == SHARED_SHADOW_NAME
            raise RuntimeError("not found")

    assert resolve_shadow_base(ShadowMissingClient(), "thing-1").value is None

    class ShadowClient:
        def get_thing_shadow(self, thing_name=None, shadow_name=None):
            payload = {
                "state": {
                    "desired": {
                        "ComponentConfig": json.dumps({"logging": {"level": "INFO"}})
                    }
                }
            }
            return SimpleNamespace(payload=json.dumps(payload).encode("utf-8"))

    assert resolve_shadow_base(ShadowClient(), "thing-1").value == {
        "logging": {"level": "INFO"}
    }

    class ShadowMalformedJsonClient:
        def get_thing_shadow(self, thing_name=None, shadow_name=None):
            payload = {
                "state": {
                    "desired": {
                        "ComponentConfig": "{ not json"
                    }
                }
            }
            return SimpleNamespace(payload=json.dumps(payload).encode("utf-8"))

    with pytest.raises(SplitConfigError) as exc:
        resolve_shadow_base(ShadowMalformedJsonClient(), "thing-1")
    assert exc.value.code == "SHARED_CONFIG_INVALID"

    class ShadowNonObjectClient:
        def get_thing_shadow(self, thing_name=None, shadow_name=None):
            payload = {
                "state": {
                    "desired": {
                        "ComponentConfig": json.dumps(["not", "object"])
                    }
                }
            }
            return SimpleNamespace(payload=json.dumps(payload).encode("utf-8"))

    with pytest.raises(SplitConfigError) as exc:
        resolve_shadow_base(ShadowNonObjectClient(), "thing-1")
    assert exc.value.code == "SHARED_CONFIG_INVALID"

    class ShadowNonStringClient:
        def get_thing_shadow(self, thing_name=None, shadow_name=None):
            payload = {
                "state": {
                    "desired": {
                        "ComponentConfig": {"logging": {"level": "INFO"}}
                    }
                }
            }
            return SimpleNamespace(payload=json.dumps(payload).encode("utf-8"))

    with pytest.raises(SplitConfigError) as exc:
        resolve_shadow_base(ShadowNonStringClient(), "thing-1")
    assert exc.value.code == "SHARED_CONFIG_INVALID"


def test_config_component_bundle_vectors():
    for case in _vectors("config-component-bundles.json"):
        body = case["input"].get("body") or case["input"].get("push")
        expected = case["expected"]
        if "error" in expected:
            with pytest.raises(SplitConfigError) as exc:
                parse_config_component_payload(body)
            assert exc.value.code == expected["error"]
            continue
        parsed = parse_config_component_payload(body)
        if "base" in expected and "component" in expected:
            assert parsed.base == expected["base"]
            assert parsed.component == expected["component"]
        else:
            effective = deep_merge_layers([parsed.base, parsed.component])
            assert effective == expected["effective"]


def test_config_component_catalog_vectors_client_owned_cases():
    for case in _vectors("config-component-catalogs.json"):
        reply = case["expected"].get("reply") if isinstance(case["expected"], dict) else None
        if reply and reply.get("ok") is False:
            with pytest.raises(SplitConfigError) as exc:
                parse_config_component_payload(reply)
            assert exc.value.code == reply["error"]["code"]

        if case["name"] == "file-loaded-catalog-derives-version":
            version = derive_catalog_version(
                case["input"]["catalog"],
                case["input"]["source"]["uri"],
            )
            assert version.startswith(case["input"]["source"]["uri"] + "#sha256:")


def test_component_extends_must_be_string_for_every_provider():
    validate_component_layer({"extends": "shared.json", "component": {}})
    with pytest.raises(SplitConfigError) as exc:
        validate_component_layer({"extends": False, "component": {}})
    assert exc.value.code == "EXTENDS_INVALID"


def test_layered_manager_startup_reload_and_reject_keep():
    cm = DictLayeredManager(
        {"component": {"global": {"component": "c"}}},
        {"logging": {"level": "INFO"}, "component": {"global": {"base": "b"}}},
    )
    assert cm.get_effective_config() == {
        "logging": {"level": "INFO"},
        "component": {"global": {"base": "b", "component": "c"}},
    }

    cm.complete_initialization()
    cm._component_layer = {"component": {"global": {"component": "next"}}}
    assert cm.reload_from_provider() is True
    assert cm.get_global_config() == {"base": "b", "component": "next"}

    cm._base_layer = {"extends": "site.json", "logging": {"level": "WARN"}}
    assert cm.reload_from_provider() is False
    assert cm.get_global_config() == {"base": "b", "component": "next"}


def test_config_component_legacy_refetch_is_component_only_but_push_preserves_base():
    initial = {
        "base": {"logging": {"level": "INFO"}, "tags": {"site": "dallas"}},
        "component": {"component": {"global": {"v": 1}}},
    }
    cm = ConfigComponentLayeredManager(initial)
    assert cm.get_effective_config()["logging"]["level"] == "INFO"

    cm.complete_initialization()
    cm._source_payload = {"component": {"global": {"v": 2}}, "tags": {"component": "reload"}}
    assert cm.reload_from_provider() is True
    assert "logging" not in cm.get_effective_config()
    assert cm.get_effective_config()["tags"] == {"component": "reload"}

    assert cm.configuration_changed(
        {"component": {"global": {"v": 3}}, "tags": {"component": "push"}}
    ) is True
    assert "logging" not in cm.get_effective_config()

    assert cm.configuration_changed(initial) is True
    assert cm.configuration_changed(
        {"component": {"global": {"v": 4}}, "tags": {"component": "push"}}
    ) is True
    assert cm.get_effective_config()["logging"]["level"] == "INFO"
    assert cm.get_effective_config()["tags"] == {"site": "dallas", "component": "push"}


def test_file_manager_rearms_shared_base_watch_when_extends_moves(tmp_path):
    base1_dir = tmp_path / "base1"
    base2_dir = tmp_path / "base2"
    base1_dir.mkdir()
    base2_dir.mkdir()
    (base1_dir / "shared.json").write_text(json.dumps({"tags": {"site": "one"}}))
    (base2_dir / "shared.json").write_text(json.dumps({"tags": {"site": "two"}}))
    config_path = tmp_path / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "extends": str(base1_dir / "shared.json"),
                "component": {"global": {"v": 1}},
            }
        )
    )

    cm = FileConfigManager("thing-1", "com.example.C", str(config_path))
    try:
        assert str(base1_dir.resolve()) in cm._watched_dirs

        config_path.write_text(
            json.dumps(
                {
                    "extends": str(base2_dir / "shared.json"),
                    "component": {"global": {"v": 2}},
                }
            )
        )
        assert cm.reload_from_provider() is True
        assert str(base2_dir.resolve()) in cm._watched_dirs
        assert cm.get_effective_config()["tags"] == {"site": "two"}
    finally:
        cm.close()


def test_configmap_manager_rearms_shared_base_watch_when_extends_moves(tmp_path):
    mount_dir = tmp_path / "cm"
    base1_dir = tmp_path / "base1"
    base2_dir = tmp_path / "base2"
    mount_dir.mkdir()
    base1_dir.mkdir()
    base2_dir.mkdir()
    (base1_dir / "shared.json").write_text(json.dumps({"tags": {"site": "one"}}))
    (base2_dir / "shared.json").write_text(json.dumps({"tags": {"site": "two"}}))
    config_path = mount_dir / "config.json"
    config_path.write_text(
        json.dumps(
            {
                "extends": str(base1_dir / "shared.json"),
                "component": {"global": {"v": 1}},
            }
        )
    )

    cm = ConfigMapConfigManager("thing-1", "com.example.C", str(mount_dir), "config.json")
    try:
        assert str(base1_dir.resolve()) in cm._base_watched_dirs

        config_path.write_text(
            json.dumps(
                {
                    "extends": str(base2_dir / "shared.json"),
                    "component": {"global": {"v": 2}},
                }
            )
        )
        assert cm.reload_from_provider() is True
        assert str(base2_dir.resolve()) in cm._base_watched_dirs
        assert cm.get_effective_config()["tags"] == {"site": "two"}
    finally:
        cm.close()


def test_layered_manager_shared_config_false_and_cli_opt_out():
    cm = DictLayeredManager(
        {"sharedConfig": False, "component": {"global": {"v": 1}}},
        {"logging": {"level": "INFO"}},
    )
    assert cm.get_effective_config() == {"component": {"global": {"v": 1}}}

    cm = DictLayeredManager(
        {"sharedConfig": True, "component": {"global": {"v": 1}}},
        {"logging": {"level": "INFO"}},
        no_shared_config=True,
    )
    assert cm.get_effective_config() == {"component": {"global": {"v": 1}}}


def test_cli_no_shared_config_flag_parses():
    from edgecommons.edgecommons import EdgeCommons

    obj = object.__new__(EdgeCommons)
    parsed = obj._process_args(
        "com.example.C",
        ["--platform", "HOST", "--no-shared-config", "-c", "FILE", "config.json"],
        None,
    )
    assert parsed.no_shared_config is True


def test_builder_passes_no_shared_config(monkeypatch):
    import edgecommons.config.manager.config_manager_builder as cmb

    calls = {}
    monkeypatch.setattr(
        cmb,
        "FileConfigManager",
        lambda *args, **kwargs: calls.update(args=args, kwargs=kwargs) or "manager",
    )
    args = Namespace(
        config=["FILE", "config.json"],
        identity="thing-1",
        thing="thing-1",
        platform=None,
        no_shared_config=True,
    )
    assert cmb.ConfigManagerBuilder.build(args, "com.example.C") == "manager"
    assert calls["kwargs"]["no_shared_config"] is True
