import copy
import json
from pathlib import Path

import pytest

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.hierarchical_config import (
    HierarchicalConfigError,
    deep_merge_layers,
    derive_catalog_version,
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


# ===================== client-side bundle/merge validation =====================
#
# The JSON vectors above pin the cross-language happy paths and the shared error cases.
# What follows pins the client-side guards the vectors do not reach: the shapes a buggy
# or hostile CONFIG_COMPONENT provider could put on the wire, and the merge semantics a
# component author depends on. Each must surface as a typed HierarchicalConfigError with
# a code -- never a raw KeyError/TypeError escaping into the component's startup path.


def _valid_bundle():
    return {
        "lineageVersion": 1,
        "catalogVersion": "sha256:cafe",
        "component": "com.example.opcua-adapter",
        "layers": [
            {"id": "site", "kind": "scope", "scope": {"site": "plant-1"},
             "config": {"a": 1}},
            {"id": "comp", "kind": "component",
             "component": "com.example.opcua-adapter", "config": {"b": 2}},
        ],
    }


def _failure(fn, *args):
    with pytest.raises(HierarchicalConfigError) as exc:
        fn(*args)
    return exc.value.code, str(exc.value)


class TestDeepMergeLayers:
    def test_absent_layers_are_skipped_rather_than_failing(self):
        assert deep_merge_layers([None, {"a": 1}, None, {"b": 2}]) == {"a": 1, "b": 2}

    def test_a_layer_that_is_not_an_object_is_a_typed_error(self):
        code, message = _failure(deep_merge_layers, [{"a": 1}, ["not", "an", "object"]])
        assert code == "CONFIG_LAYER_INVALID"
        assert "JSON object" in message

    def test_later_layers_win_and_inputs_are_never_mutated(self):
        base = {"nested": {"keep": 1, "override": "old"}, "arr": [1, 2]}
        top = {"nested": {"override": "new"}, "arr": [3]}

        merged = deep_merge_layers([base, top])

        assert merged == {"nested": {"keep": 1, "override": "new"}, "arr": [3]}, (
            "objects merge recursively; arrays replace wholesale"
        )
        assert base == {"nested": {"keep": 1, "override": "old"}, "arr": [1, 2]}, (
            "the caller's layer must not be mutated"
        )

    def test_a_type_conflict_is_reported_to_the_warn_hook_with_both_json_types(self):
        # An operator overriding a string with a number (etc.) is legal but almost always
        # a mistake -- this hook is how the component surfaces it.
        seen = []

        deep_merge_layers(
            [
                {"port": "1883", "debug": 1, "retries": True},
                {"port": 1883, "debug": False, "retries": "3"},
            ],
            warn=lambda path, left, right: seen.append((path, left, right)),
        )

        assert ("$.port", "string", "number") in seen
        assert ("$.debug", "number", "boolean") in seen
        assert ("$.retries", "boolean", "string") in seen

    def test_arrays_and_absent_values_do_not_trip_the_conflict_warning(self):
        seen = []

        deep_merge_layers(
            [{"arr": [1], "gone": None}, {"arr": {"now": "object"}, "gone": "value"}],
            warn=lambda *a: seen.append(a),
        )

        assert seen == [], "list/null transitions are legal replacements, not conflicts"


class TestParseConfigComponentPayloadRejectsBadBundles:
    def test_a_json_string_payload_is_parsed(self):
        parsed = parse_config_component_payload(json.dumps(_valid_bundle()))
        assert parsed.catalog_version == "sha256:cafe"
        assert parsed.component == "com.example.opcua-adapter"
        assert parsed.configs == [{"a": 1}, {"b": 2}]

    def test_a_malformed_json_string_is_a_typed_error(self):
        code, message = _failure(parse_config_component_payload, "{not json")
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "malformed JSON" in message

    @pytest.mark.parametrize("payload", [["a", "list"], 42, None])
    def test_a_payload_that_is_not_an_object_is_a_typed_error(self, payload):
        code, _ = _failure(parse_config_component_payload, payload)
        assert code == "LINEAGE_BUNDLE_INVALID"

    def test_a_structured_provider_error_surfaces_its_own_code(self):
        code, message = _failure(
            parse_config_component_payload,
            {"ok": False,
             "error": {"code": "CATALOG_UNAVAILABLE", "message": "no catalog"}},
        )
        assert code == "CATALOG_UNAVAILABLE"
        assert message == "no catalog"

    def test_an_unversioned_bundle_is_rejected(self):
        bundle = _valid_bundle()
        del bundle["lineageVersion"]
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "lineageVersion 1" in message

    @pytest.mark.parametrize("catalog_version", ["", 1, None])
    def test_a_bundle_without_a_usable_catalog_version_is_rejected(self, catalog_version):
        # The catalog version pins a deployment to an exact config; a blank one would
        # make that pin unverifiable.
        bundle = _valid_bundle()
        bundle["catalogVersion"] = catalog_version
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "catalogVersion" in message

    @pytest.mark.parametrize("component", ["", 7, None])
    def test_a_bundle_without_a_usable_component_is_rejected(self, component):
        bundle = _valid_bundle()
        bundle["component"] = component
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "component" in message

    def test_a_layer_that_is_not_an_object_is_rejected_with_its_index(self):
        bundle = _valid_bundle()
        bundle["layers"] = ["nope", bundle["layers"][1]]
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "index 0" in message

    def test_the_component_layer_must_be_final(self):
        # A lineage whose last word is a scope would let a broader scope override the
        # component's own config -- precedence inverted.
        bundle = _valid_bundle()
        bundle["layers"] = [bundle["layers"][1], bundle["layers"][0]]
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "component layer must be final" in message

    def test_a_lineage_that_never_reaches_the_component_is_rejected(self):
        bundle = _valid_bundle()
        bundle["layers"] = [bundle["layers"][0]]
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "final layer must be kind 'component'" in message

    def test_a_component_layer_naming_a_different_component_is_rejected(self):
        bundle = _valid_bundle()
        bundle["layers"][1]["component"] = "com.example.other"
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "does not match bundle component" in message

    def test_a_component_layer_scope_must_still_be_an_object_when_present(self):
        bundle = _valid_bundle()
        bundle["layers"][1]["scope"] = "plant-1"
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "scope must be a JSON object when present" in message

    def test_a_scope_layer_without_an_object_scope_is_rejected(self):
        bundle = _valid_bundle()
        bundle["layers"][0]["scope"] = "plant-1"
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "must contain object scope" in message

    @pytest.mark.parametrize("kind", ["site", "", None])
    def test_an_unknown_layer_kind_is_rejected(self, kind):
        bundle = _valid_bundle()
        bundle["layers"][0]["kind"] = kind
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "kind 'scope' or 'component'" in message

    def test_a_layer_config_that_is_not_an_object_is_rejected(self):
        bundle = _valid_bundle()
        bundle["layers"][0]["config"] = ["a"]
        code, message = _failure(parse_config_component_payload, bundle)
        assert code == "LINEAGE_BUNDLE_INVALID"
        assert "config must be a JSON object" in message

    def test_the_parsed_layers_are_copies_so_a_caller_cannot_mutate_the_bundle(self):
        bundle = _valid_bundle()
        parsed = parse_config_component_payload(bundle)

        parsed.layers[0]["config"]["a"] = "tampered"

        assert bundle["layers"][0]["config"]["a"] == 1


class TestDeriveCatalogVersion:
    def test_it_is_a_stable_content_digest_independent_of_key_order(self):
        one = derive_catalog_version({"a": 1, "b": 2}, "")
        two = derive_catalog_version({"b": 2, "a": 1}, "")
        assert one == two, "the digest must not depend on dict ordering"
        assert one.startswith("sha256:")

    def test_a_different_catalog_yields_a_different_digest(self):
        assert derive_catalog_version({"a": 1}, "") != derive_catalog_version({"a": 2}, "")

    def test_a_source_uri_is_prefixed_onto_the_digest(self):
        version = derive_catalog_version({"a": 1}, "s3://bucket/catalog.json")
        assert version.startswith("s3://bucket/catalog.json#sha256:")
