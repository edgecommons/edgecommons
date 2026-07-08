import copy
import json
from pathlib import Path

import pytest

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.hierarchical_config import (
    HierarchicalConfigError,
    deep_merge_layers,
    parse_config_component_payload,
)


VECTOR_DIR = Path(__file__).resolve().parents[3] / "hierarchical-config-test-vectors"


def _vectors(name):
    return json.loads((VECTOR_DIR / name).read_text())["cases"]


class DirectDocumentManager(ConfigManager):
    def __init__(self, source_payload, **kwargs):
        self._source_payload = source_payload
        super().__init__(
            "com.example.opcua-adapter",
            "thing-1",
            validate_config=kwargs.pop("validate_config", False),
            **kwargs,
        )
        self._config_provider_family = "FILE"
        self._config_source = "FILE"
        self.init()

    def _load_configuration(self):
        return self._source_payload


class ConfigComponentLineageManager(ConfigManager):
    def __init__(self, source_payload, **kwargs):
        self._source_payload = source_payload
        super().__init__(
            "com.example.opcua-adapter",
            "thing-1",
            validate_config=kwargs.pop("validate_config", False),
            **kwargs,
        )
        self._config_provider_family = "CONFIG_COMPONENT"
        self._config_source = "CONFIG_COMPONENT"
        self.init()

    def _load_configuration(self):
        return self._source_payload


class RecordingListener:
    def __init__(self):
        self.calls = []

    def on_configuration_change(self, new_config):
        self.calls.append(copy.deepcopy(new_config))
        return True


def test_hierarchical_merge_vectors():
    for case in _vectors("merge.json"):
        warnings = []
        effective = deep_merge_layers(
            [layer["config"] for layer in case["input"]["layers"]],
            lambda path, left, right: warnings.append(
                {"path": path, "code": "TYPE_CONFLICT_LATER_LAYER_WINS"}
            ),
        )
        assert effective == case["expected"]["effective"], case["name"]
        if "warnings" in case["expected"]:
            assert warnings == case["expected"]["warnings"], case["name"]


def test_config_component_lineage_bundle_vectors():
    for case in _vectors("lineage-bundles.json"):
        body = case["input"]["body"]
        expected = case["expected"]
        request_component = case["input"].get("requestComponent")
        if "error" in expected:
            with pytest.raises(HierarchicalConfigError) as exc:
                parse_config_component_payload(body, request_component=request_component)
            assert exc.value.code == expected["error"], case["name"]
            continue

        parsed = parse_config_component_payload(body, request_component=request_component)
        effective = deep_merge_layers(parsed.configs)
        assert effective == expected["effective"], case["name"]


def test_config_component_manager_applies_lineage_effective_only():
    body = _vectors("lineage-bundles.json")[0]["input"]["body"]
    cm = ConfigComponentLineageManager(body)
    assert cm.get_effective_config() == _vectors("lineage-bundles.json")[0]["expected"]["effective"]
    assert cm.get_global_config() == {
        "endpoint": "opc.tcp://10.10.7.20:4840",
        "publish_interval": 3,
    }
    assert cm._latest_base_layer is None


def test_config_component_reload_error_vectors_reject_and_keep():
    valid_push = next(c for c in _vectors("errors.json") if c["name"] == "valid-push-replaces-previous-effective")
    initial = valid_push["input"]["push"]
    cm = ConfigComponentLineageManager(initial, validate_config=True)
    cm.complete_initialization()
    listener = RecordingListener()
    cm.add_config_change_listener(listener)

    for case in _vectors("errors.json"):
        if case["name"] == "valid-push-replaces-previous-effective":
            continue
        body = case["input"].get("push") or case["input"].get("body")
        previous = copy.deepcopy(cm.get_effective_config())
        assert cm.configuration_changed(body) is False, case["name"]
        assert cm.get_effective_config() == previous, case["name"]
        assert listener.calls == [], case["name"]

    assert cm.configuration_changed(valid_push["input"]["push"]) is True
    assert cm.get_effective_config() == valid_push["expected"]["effective"]
    assert listener.calls == [valid_push["expected"]["effective"]]


def test_direct_provider_payload_is_single_effective_document():
    payload = {
        "unknownTopLevel": {"retained": True},
        "component": {"token": "opcua-adapter", "global": {"v": 1}},
    }
    cm = DirectDocumentManager(payload)
    assert cm.get_effective_config() == payload

    cm.complete_initialization()
    cm._source_payload = {
        "component": {"token": "opcua-adapter", "global": {"v": 2}},
        "tags": {"source": "direct"},
    }
    assert cm.reload_from_provider() is True
    assert cm.get_effective_config() == cm._source_payload


def test_cli_no_shared_config_flag_is_removed():
    from edgecommons.edgecommons import EdgeCommons

    obj = object.__new__(EdgeCommons)
    with pytest.raises(SystemExit):
        obj._process_args(
            "com.example.C",
            ["--platform", "HOST", "--no-shared-config", "-c", "FILE", "config.json"],
            None,
        )
