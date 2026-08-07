"""Numeric canonicalization at the configuration intake boundary (D-NC1 / D-NC2).

Two layers:

1. **The pass itself** — the normative §3 semantics table from
   ``docs/platform/DESIGN-config-numeric-canonicalization.md``, plus nesting, key immutability,
   idempotency, purity, and the strict readers' rejection messages (which mirror the canonical
   Java ``ConfigNumbers``).
2. **The pipeline** — the Greengrass delivery shape reproduced with **no device**: the
   ``Map<String, Object>`` the IPC SDK hands back, whose numbers are Python ``float`` because the
   Nucleus configuration store round-trips every JSON number through a Java ``double``, pushed
   through the real ``GreengrassConfigManager`` intake and through the source-agnostic
   ``ConfigManager`` intake. The delivered document must carry ``int`` where the value is
   integral, and a genuinely fractional value must survive untouched as a ``float``.
"""
import copy
import json
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import pytest

from edgecommons.config import canonicalize_json_numbers
from edgecommons.config.canonicalize import (
    MAX_SIGNED_32,
    MAX_UNSIGNED_64,
    as_integral_int,
    require_non_negative_int,
    require_non_negative_integral,
)
from edgecommons.config.candidate_validation import (
    ConfigurationValidationPhase,
    ConfigurationValidationResult,
)
from edgecommons.config.effective_config_publisher import redact
from edgecommons.config.enhanced_logging_config import EnhancedLoggingConfiguration
from edgecommons.config.health_config import HealthConfiguration
from edgecommons.config.heartbeat_config import HeartbeatConfiguration
from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.metric_config import MetricConfiguration
from edgecommons.validation.configuration_validator import (
    ConfigurationValidationException,
    ConfigurationValidator,
)

#: 2^64 — the exclusive upper bound of the unsigned window. The largest ``float`` strictly below
#: it is ``2^64 - 2048``; a naive round-trip check would wrongly accept 2^64 itself.
TWO_TO_THE_64 = 2.0 ** 64


# ===========================================================================================
# 1. The pass — the §3 semantics table.
# ===========================================================================================


class TestCanonicalizationSemantics:
    """Every row of the §3 table, on the same values the Java and Rust suites use."""

    def test_an_integral_float_becomes_an_integer(self):
        for source, expected in ((5000.0, 5000), (0.0, 0), (1.0, 1)):
            result = canonicalize_json_numbers(source)
            assert result == expected
            assert isinstance(result, int)

    def test_an_integer_is_a_fixed_point(self):
        for source in (5000, -5, MAX_UNSIGNED_64, -(2 ** 63), 2 ** 70):
            result = canonicalize_json_numbers(source)
            assert result == source
            assert isinstance(result, int)

    def test_a_fractional_value_is_left_untouched(self):
        for source in (5000.5, 0.1, -2.5):
            result = canonicalize_json_numbers(source)
            assert result == source
            assert isinstance(result, float)

    def test_a_negative_integral_float_becomes_a_signed_integer(self):
        result = canonicalize_json_numbers(-5.0)
        assert result == -5
        assert isinstance(result, int)

    def test_negative_zero_becomes_zero(self):
        result = canonicalize_json_numbers(-0.0)
        assert result == 0
        assert isinstance(result, int)

    def test_large_integral_floats_inside_the_unsigned_window_convert(self):
        assert canonicalize_json_numbers(1e19) == 10_000_000_000_000_000_000
        assert isinstance(canonicalize_json_numbers(1e19), int)
        # 2^63 exactly — above the signed window, inside the unsigned one.
        assert canonicalize_json_numbers(2.0 ** 63) == 9_223_372_036_854_775_808
        # 2^64 - 2048, the largest float strictly below 2^64.
        largest = TWO_TO_THE_64 - 2048.0
        assert canonicalize_json_numbers(largest) == 18_446_744_073_709_549_568
        assert isinstance(canonicalize_json_numbers(largest), int)

    def test_values_outside_the_sixty_four_bit_window_stay_floats(self):
        for source in (1e20, TWO_TO_THE_64, -TWO_TO_THE_64):
            result = canonicalize_json_numbers(source)
            assert result == source
            assert isinstance(result, float), f"{source} must stay a float"

    def test_the_negative_window_bound_converts_exactly(self):
        result = canonicalize_json_numbers(-(2.0 ** 63))
        assert result == -(2 ** 63)
        assert isinstance(result, int)

    def test_the_two_to_the_fifty_third_boundary_is_exact(self):
        assert canonicalize_json_numbers(9_007_199_254_740_992.0) == 9_007_199_254_740_992
        # 2^53 + 1 can only arrive as an integer literal; it stays untouched.
        assert canonicalize_json_numbers(9_007_199_254_740_993) == 9_007_199_254_740_993

    def test_non_finite_values_are_left_untouched(self):
        import math

        for source in (float("nan"), float("inf"), float("-inf")):
            result = canonicalize_json_numbers(source)
            assert isinstance(result, float)
            assert math.isnan(result) if math.isnan(source) else result == source

    def test_strings_and_none_are_never_coerced(self):
        # D-NC4: coercing a numeric-looking string would corrupt legitimately-string settings.
        for source in ("5000", "5000.0", "", None):
            assert canonicalize_json_numbers(source) == source
        assert isinstance(canonicalize_json_numbers("5000"), str)

    def test_booleans_are_never_coerced(self):
        # Python-specific trap: bool subclasses int, so a naive numeric isinstance check would
        # treat True as the number 1 and rewrite it.
        for source in (True, False):
            result = canonicalize_json_numbers(source)
            assert result is source
            assert isinstance(result, bool)

    def test_booleans_inside_a_document_stay_booleans(self):
        result = canonicalize_json_numbers({"enabled": True, "disabled": False})
        assert result == {"enabled": True, "disabled": False}
        assert isinstance(result["enabled"], bool)
        assert isinstance(result["disabled"], bool)


class TestCanonicalizationStructure:
    def test_nested_objects_and_arrays_are_walked(self):
        result = canonicalize_json_numbers(
            {"a": {"b": [1.0, {"c": 2.0}, [3.0]]}, "d": 4.0}
        )
        assert result == {"a": {"b": [1, {"c": 2}, [3]]}, "d": 4}
        assert isinstance(result["a"]["b"][1]["c"], int)
        assert isinstance(result["a"]["b"][2][0], int)

    def test_keys_are_never_touched(self):
        result = canonicalize_json_numbers({"5000.0": 1.0, "": 2.0})
        assert list(result) == ["5000.0", ""]

    def test_the_pass_is_idempotent(self):
        source = {
            "numbers": [5000.0, -5.0, 5000.5, 1e20, "5000", True, None],
            "nested": {"deep": {"v": 30.0}},
        }
        once = canonicalize_json_numbers(source)
        twice = canonicalize_json_numbers(once)
        assert once == twice
        assert json.dumps(once) == json.dumps(twice)

    def test_the_callers_document_is_never_mutated(self):
        source = {"component": {"instances": [{"pollIntervalMs": 5000.0}]}}
        untouched = copy.deepcopy(source)
        result = canonicalize_json_numbers(source)
        assert source == untouched
        assert isinstance(source["component"]["instances"][0]["pollIntervalMs"], float)
        # ... and the result is a genuine copy, not a view onto the argument.
        result["component"]["instances"][0]["pollIntervalMs"] = 1
        assert source["component"]["instances"][0]["pollIntervalMs"] == 5000.0

    def test_an_empty_document_is_unchanged(self):
        assert canonicalize_json_numbers({}) == {}
        assert canonicalize_json_numbers([]) == []


# ===========================================================================================
# 2. The strict readers (D-NC2) — wording mirrors the canonical Java ConfigNumbers.
# ===========================================================================================


class TestStrictReaders:
    def test_both_encodings_of_an_integral_value_are_accepted(self):
        for source in (5, 5.0, -0.0, 0):
            assert isinstance(require_non_negative_int(source, "p"), int)
        assert require_non_negative_int(5.0, "p") == 5
        assert require_non_negative_integral(300.0, "p") == 300

    @pytest.mark.parametrize(
        "value, expected",
        [
            (5.5, "configuration value 'heartbeat.intervalSecs' must be a whole number, but was 5.5"),
            (-5, "configuration value 'heartbeat.intervalSecs' must not be negative, but was -5"),
            (-5.0, "configuration value 'heartbeat.intervalSecs' must not be negative, but was -5.0"),
            ("5", 'configuration value \'heartbeat.intervalSecs\' must be a number, but was "5"'),
            (True, "configuration value 'heartbeat.intervalSecs' must be a number, but was true"),
            (None, "configuration value 'heartbeat.intervalSecs' must be a number, but was null"),
            ({}, "configuration value 'heartbeat.intervalSecs' must be a number, but was an object"),
            ([], "configuration value 'heartbeat.intervalSecs' must be a number, but was an array"),
            (
                float("nan"),
                "configuration value 'heartbeat.intervalSecs' must be a finite number, but was nan",
            ),
        ],
    )
    def test_rejections_name_the_offending_value(self, value, expected):
        with pytest.raises(ValueError) as excinfo:
            require_non_negative_int(value, "heartbeat.intervalSecs")
        assert str(excinfo.value) == expected

    def test_a_value_of_an_unexpected_type_is_still_described(self):
        # Nothing a JSON document can hold, but the renderer must never itself raise.
        with pytest.raises(ValueError) as excinfo:
            require_non_negative_integral({"a", "b"}, "health.port")
        assert "configuration value 'health.port' must be a number, but was " in str(
            excinfo.value
        )

    def test_a_value_beyond_the_32_bit_range_is_rejected(self):
        with pytest.raises(ValueError) as excinfo:
            require_non_negative_int(MAX_SIGNED_32 + 1, "health.port")
        assert str(excinfo.value) == (
            "configuration value 'health.port' is out of range for a 32-bit integer: 2147483648"
        )
        # ... while the 64-bit reader accepts it.
        assert require_non_negative_integral(MAX_SIGNED_32 + 1, "health.port") == 2147483648

    def test_a_value_beyond_the_64_bit_range_is_rejected(self):
        with pytest.raises(ValueError) as excinfo:
            require_non_negative_integral(2 ** 64, "credentials.vault.keepVersions")
        assert "is out of range for a 64-bit integer: 18446744073709551616" in str(excinfo.value)

    def test_the_probe_falls_back_rather_than_truncating(self):
        assert as_integral_int(5) == 5
        assert as_integral_int(5.0) == 5
        assert as_integral_int(5.5) is None
        assert as_integral_int(-5.0) is None
        assert as_integral_int(1e20) is None
        assert as_integral_int("5") is None
        assert as_integral_int(True) is None
        assert as_integral_int(None) is None


# ===========================================================================================
# 3. The pipeline — the Greengrass delivery shape, reproduced with no device.
# ===========================================================================================


def greengrass_ipc_configuration():
    """The value the Greengrass IPC SDK returns from ``get_configuration()``.

    The Nucleus configuration store keeps JSON numbers as Java ``Double``, so every configured
    integer arrives here as a Python ``float``: a deployed ``pollIntervalMs: 5000`` is ``5000.0``.
    ``ratio`` is genuinely fractional and must stay a float; ``line`` is a numeric tag (the schema
    types tags as strings, so it is only reachable on a path that bypasses the schema gate).
    """
    return {
        "ComponentConfig": {
            "heartbeat": {"intervalSecs": 10.0},
            "health": {"port": 8082.0},
            "logging": {
                "fileLogging": {"enabled": False, "backupCount": 3.0},
                "publish": {
                    "enabled": True,
                    "maxRecordBytes": 4096.0,
                    "queue": {"maxRecords": 500.0, "onFull": "dropOldest"},
                },
            },
            "metricEmission": {
                "target": "cloudwatch",
                "targetConfig": {"intervalSecs": 15.0},
            },
            "messaging": {"requestTimeoutSeconds": 45.0},
            "component": {
                "global": {"healthThresholds": {"staleSignalSecs": 30.0}},
                "instances": [
                    {
                        "id": "plc-1",
                        "pollIntervalMs": 5000.0,
                        "slot": 0.0,
                        "timeoutMs": 2500.0,
                        "scaleFactor": 0.5,
                        "enabled": True,
                        "name": "press-1",
                    }
                ],
            },
        }
    }


class DictConfigManager(ConfigManager):
    """A config manager whose source is a fixed document — the source-agnostic intake."""

    def __init__(self, document, **kwargs):
        self._document = document
        kwargs.setdefault("thing_name", "thing-1")
        super().__init__("com.example.Adapter", **kwargs)
        self.init()

    def _load_configuration(self):
        return self._document


def greengrass_config_manager(ipc_value):
    """A real :class:`GreengrassConfigManager` driven by a stubbed IPC client.

    Everything below the client is the production path: ``_load_configuration`` reads the
    component's key out of the response, ``init()`` builds the effective document, the schema
    gate runs, and the snapshot is committed.
    """
    from edgecommons.config.manager import greengrass_config_manager as module

    client = MagicMock()
    client.get_configuration.return_value = SimpleNamespace(value=ipc_value)
    with patch.object(module, "GreengrassCoreIPCClientV2", return_value=client):
        return module.GreengrassConfigManager(
            thing_name="thing-1",
            component_name="com.example.Adapter",
            config_component_name=None,
            config_key="ComponentConfig",
        )


class TestGreengrassShapedDelivery:
    """The reported defect, reproduced and fixed without a device."""

    def test_the_greengrass_config_source_delivers_integers(self):
        manager = greengrass_config_manager(greengrass_ipc_configuration())

        instance = manager.get_instance_config("plc-1")
        assert instance["pollIntervalMs"] == 5000
        assert isinstance(instance["pollIntervalMs"], int)
        assert isinstance(instance["timeoutMs"], int)
        assert isinstance(instance["slot"], int)
        assert isinstance(
            manager.get_global_config()["healthThresholds"]["staleSignalSecs"], int
        )

    def test_a_genuinely_fractional_value_survives_as_a_float(self):
        manager = greengrass_config_manager(greengrass_ipc_configuration())
        instance = manager.get_instance_config("plc-1")
        assert instance["scaleFactor"] == 0.5
        assert isinstance(instance["scaleFactor"], float)

    def test_component_booleans_and_strings_are_untouched(self):
        manager = greengrass_config_manager(greengrass_ipc_configuration())
        instance = manager.get_instance_config("plc-1")
        assert instance["enabled"] is True
        assert instance["name"] == "press-1"

    def test_a_strict_typed_consumer_parses_the_delivered_instance_subtree(self):
        # The shape a scaffolded adapter parses each component.instances[] entry with: an
        # integer-typed field read straight off the delivered document. This is the read that
        # produced `invalid type: floating point 5000.0, expected u64` on the device; in Python
        # the same drift is silent, so the assertion is on the delivered type.
        manager = greengrass_config_manager(greengrass_ipc_configuration())
        instance = manager.get_instance_config("plc-1")

        class DeviceConfig:
            def __init__(self, raw):
                if not isinstance(raw["pollIntervalMs"], int):
                    raise TypeError(
                        f"invalid device config: expected int, got {type(raw['pollIntervalMs'])}"
                    )
                self.poll_interval_ms = raw["pollIntervalMs"]

        assert DeviceConfig(instance).poll_interval_ms == 5000
        # ... and the same struct still refuses the raw store document.
        with pytest.raises(TypeError):
            DeviceConfig(greengrass_ipc_configuration()["ComponentConfig"]["component"]["instances"][0])

    def test_the_library_sections_parse_the_store_shape(self):
        manager = greengrass_config_manager(greengrass_ipc_configuration())
        assert manager.get_heartbeat_config().get_interval_secs() == 10
        assert isinstance(manager.get_heartbeat_config().get_interval_secs(), int)
        assert manager.get_health_config().port == 8082
        assert isinstance(manager.get_health_config().port, int)
        assert manager.get_metric_config().get_interval_secs() == 15
        publish = manager.get_logging_config().get_publish_config()
        assert publish.max_record_bytes == 4096
        assert publish.queue.max_records == 500
        assert manager.get_messaging_request_timeout() == 45.0

    def test_the_full_and_effective_documents_are_canonical(self):
        manager = greengrass_config_manager(greengrass_ipc_configuration())
        for document in (manager.get_full_config(), manager.get_effective_config()):
            assert document["heartbeat"]["intervalSecs"] == 10
            assert isinstance(document["heartbeat"]["intervalSecs"], int)
            assert isinstance(
                document["component"]["instances"][0]["pollIntervalMs"], int
            )
        # The published `cfg` document is the redacted effective config — canonical too.
        published = redact(manager.get_effective_config())
        assert isinstance(published["component"]["instances"][0]["pollIntervalMs"], int)
        assert '"pollIntervalMs": 5000' in json.dumps(published, indent=1)

    def test_a_poisoned_store_needs_no_reset(self):
        # The Greengrass configuration store is cumulative: once a config update has written
        # 5000.0 into it, redeploying the untouched recipe does not clear it — only an explicit
        # RESET did. The fix is read-side and unconditional, so a component started against a
        # store that is *already* poisoned reads it correctly, with no reset and without
        # rewriting the store.
        poisoned_store = greengrass_ipc_configuration()
        manager = greengrass_config_manager(poisoned_store)
        assert manager.get_instance_config("plc-1")["pollIntervalMs"] == 5000
        assert isinstance(manager.get_instance_config("plc-1")["pollIntervalMs"], int)
        # The store itself is untouched — nothing wrote back to it.
        assert isinstance(
            poisoned_store["ComponentConfig"]["component"]["instances"][0]["pollIntervalMs"],
            float,
        )

    def test_a_config_update_against_a_poisoned_store_is_applied(self):
        # The production mechanism that killed the component: the store is already full of
        # doubles and a config update arrives (also as doubles).
        manager = greengrass_config_manager(greengrass_ipc_configuration())
        updated = greengrass_ipc_configuration()["ComponentConfig"]
        updated["component"]["instances"][0]["pollIntervalMs"] = 7500.0

        assert manager.configuration_changed(updated) is True
        assert manager.get_instance_config("plc-1")["pollIntervalMs"] == 7500
        assert isinstance(manager.get_instance_config("plc-1")["pollIntervalMs"], int)
        assert manager.get_generation() == 2


class TestIntakeInvariants:
    """The document every gate sees is the same canonical document."""

    def test_the_schema_gate_accepts_the_store_shape_and_still_rejects_a_fractional(self):
        ConfigurationValidator.validate(
            greengrass_ipc_configuration()["ComponentConfig"]
        )
        poisoned = greengrass_ipc_configuration()["ComponentConfig"]
        poisoned["heartbeat"]["intervalSecs"] = 10.5
        with pytest.raises(ConfigurationValidationException):
            ConfigurationValidator.validate(poisoned)

    def test_candidate_validators_observe_the_canonical_document(self):
        seen = {}

        def validator(candidate, current, phase):
            seen[phase] = candidate
            return ConfigurationValidationResult(accepted=True)

        manager = DictConfigManager(
            greengrass_ipc_configuration()["ComponentConfig"],
            candidate_validators={"numbers": validator},
        )
        initial = seen[ConfigurationValidationPhase.INITIAL]
        assert isinstance(initial["component"]["instances"][0]["pollIntervalMs"], int)

        assert manager.configuration_changed(
            greengrass_ipc_configuration()["ComponentConfig"]
        )
        reload_candidate = seen[ConfigurationValidationPhase.RELOAD]
        assert isinstance(
            reload_candidate["component"]["instances"][0]["pollIntervalMs"], int
        )

    def test_change_listeners_observe_the_canonical_document(self):
        received = []
        manager = DictConfigManager(greengrass_ipc_configuration()["ComponentConfig"])
        manager.complete_initialization()

        class Listener:
            def on_configuration_change(self, config):
                received.append(config)
                return True

        manager.add_config_change_listener(Listener())
        assert manager.configuration_changed(
            greengrass_ipc_configuration()["ComponentConfig"]
        )
        assert isinstance(received[0]["component"]["instances"][0]["pollIntervalMs"], int)

    def test_tags_are_canonical_so_the_same_config_types_the_same_everywhere(self):
        # D-NC5. The canonical schema types every tag value as a string, so a numeric tag never
        # survives a schema-validated document (asserted below); canonicalizing the whole
        # document is defense for the paths that bypass the schema gate.
        document = greengrass_ipc_configuration()["ComponentConfig"]
        document["tags"] = {"site": "dallas", "line": 3.0, "ratio": 0.5}
        with pytest.raises(ConfigurationValidationException):
            ConfigurationValidator.validate(document)

        manager = DictConfigManager(document, validate_config=False)
        tags = manager.get_tag_config().to_dict()
        assert tags["line"] == 3
        assert isinstance(tags["line"], int)
        assert tags["ratio"] == 0.5
        assert tags["site"] == "dallas"

    def test_a_canonical_document_is_a_fixed_point_of_the_intake(self):
        once = DictConfigManager(
            greengrass_ipc_configuration()["ComponentConfig"]
        ).get_effective_config()
        twice = DictConfigManager(once).get_effective_config()
        assert json.dumps(once, sort_keys=True) == json.dumps(twice, sort_keys=True)

    def test_a_fractional_value_in_a_component_field_survives_untouched(self):
        document = greengrass_ipc_configuration()["ComponentConfig"]
        document["component"]["instances"][0]["pollIntervalMs"] = 5000.5
        manager = DictConfigManager(document)
        delivered = manager.get_instance_config("plc-1")["pollIntervalMs"]
        assert delivered == 5000.5
        assert isinstance(delivered, float)

    def test_a_config_component_lineage_payload_is_canonicalized_before_the_merge(self):
        # A lineage bundle is canonicalized envelope-first, so every layer fragment is canonical
        # before the deep merge and the merged effective document is canonical too.
        payload = {
            "lineageVersion": 1,
            "catalogVersion": "sha256:cafe",
            "component": "Adapter",
            "layers": [
                {
                    "id": "site",
                    "kind": "scope",
                    "scope": {"site": "dallas"},
                    "config": {"heartbeat": {"intervalSecs": 20.0}},
                },
                {
                    "id": "comp",
                    "kind": "component",
                    "component": "Adapter",
                    "config": {
                        "component": {
                            "global": {},
                            "instances": [{"id": "plc-1", "pollIntervalMs": 5000.0}],
                        }
                    },
                },
            ],
        }

        class LineageConfigManager(ConfigManager):
            def __init__(self, document):
                self._document = document
                super().__init__("com.example.Adapter", "thing-1")
                self._config_provider_family = "CONFIG_COMPONENT"
                self.init()

            def _load_configuration(self):
                return self._document

        manager = LineageConfigManager(payload)
        assert isinstance(manager.get_instance_config("plc-1")["pollIntervalMs"], int)
        assert manager.get_heartbeat_config().get_interval_secs() == 20
        # The layer the manager retains for the lineage view is canonical as well.
        assert isinstance(
            manager._latest_component_layer["component"]["instances"][0]["pollIntervalMs"],
            int,
        )


# ===========================================================================================
# 4. D-NC2 at the library's own config sections.
# ===========================================================================================


class TestLibrarySectionsRejectRatherThanRewrite:
    def test_heartbeat_interval_accepts_both_encodings(self):
        assert HeartbeatConfiguration({"intervalSecs": 7}).get_interval_secs() == 7
        assert HeartbeatConfiguration({"intervalSecs": 7.0}).get_interval_secs() == 7
        # Below the minimum still falls back to the schema default, as before.
        assert HeartbeatConfiguration({"intervalSecs": 0}).get_interval_secs() == 5

    def test_heartbeat_interval_rejects_a_fractional_instead_of_keeping_a_float(self):
        with pytest.raises(ValueError) as excinfo:
            HeartbeatConfiguration({"intervalSecs": 5.5})
        assert str(excinfo.value) == (
            "configuration value 'heartbeat.intervalSecs' must be a whole number, but was 5.5"
        )

    def test_heartbeat_interval_rejects_a_negative_instead_of_defaulting(self):
        with pytest.raises(ValueError) as excinfo:
            HeartbeatConfiguration({"intervalSecs": -5})
        assert "must not be negative, but was -5" in str(excinfo.value)

    def test_health_port_rejects_a_fractional_instead_of_truncating(self):
        assert HealthConfiguration({"port": 8082.0}).port == 8082
        with pytest.raises(ValueError) as excinfo:
            HealthConfiguration({"port": 8082.7})
        assert str(excinfo.value) == (
            "configuration value 'health.port' must be a whole number, but was 8082.7"
        )

    def test_metric_prometheus_port_rejects_a_fractional_instead_of_truncating(self):
        assert (
            MetricConfiguration({"target": "prometheus", "targetConfig": {"port": 9091.0}})
            .get_prometheus_port()
            == 9091
        )
        with pytest.raises(ValueError) as excinfo:
            MetricConfiguration({"target": "prometheus", "targetConfig": {"port": 9091.5}})
        assert "'metricEmission.targetConfig.port' must be a whole number" in str(excinfo.value)

    def test_metric_interval_secs_rejects_a_fractional_instead_of_truncating(self):
        assert (
            MetricConfiguration(
                {"target": "cloudwatch", "targetConfig": {"intervalSecs": 60.0}}
            ).get_interval_secs()
            == 60
        )
        with pytest.raises(ValueError) as excinfo:
            MetricConfiguration(
                {"target": "cloudwatch", "targetConfig": {"intervalSecs": 60.5}}
            )
        assert "'metricEmission.targetConfig.intervalSecs' must be a whole number" in str(
            excinfo.value
        )

    def test_logging_backup_count_accepts_both_encodings_and_rejects_a_fractional(self):
        config = EnhancedLoggingConfiguration({"fileLogging": {"backupCount": 3.0}})
        assert config.to_dict()["fileLogging"]["backupCount"] == 3
        with pytest.raises(ValueError) as excinfo:
            EnhancedLoggingConfiguration({"fileLogging": {"backupCount": 3.5}})
        assert "'logging.fileLogging.backupCount' must be a whole number" in str(excinfo.value)

    def test_log_publish_sizes_accept_the_store_shape(self):
        # Before canonicalization these were int-only, so a store that delivers doubles failed
        # the whole logging section — a second, quieter face of the same defect.
        config = EnhancedLoggingConfiguration(
            {"publish": {"enabled": True, "maxRecordBytes": 4096.0,
                         "queue": {"maxRecords": 500.0, "onFull": "dropOldest"}}}
        )
        publish = config.get_publish_config()
        assert publish.max_record_bytes == 4096
        assert isinstance(publish.max_record_bytes, int)
        assert publish.queue.max_records == 500
        assert isinstance(publish.queue.max_records, int)

    def test_log_publish_sizes_reject_a_fractional_and_a_non_positive(self):
        with pytest.raises(ValueError) as excinfo:
            EnhancedLoggingConfiguration({"publish": {"maxRecordBytes": 4096.5}})
        assert "'logging.publish.maxRecordBytes' must be a whole number" in str(excinfo.value)
        with pytest.raises(ValueError) as excinfo:
            EnhancedLoggingConfiguration({"publish": {"maxRecordBytes": 0}})
        assert "'logging.publish.maxRecordBytes' must be positive" in str(excinfo.value)

    def test_the_cloudwatch_buffer_probes_fall_back_rather_than_truncating(self):
        from edgecommons.metrics.targets.cloudwatch import CloudWatch, _DEFAULT_MAX_DISK_BYTES

        manager = MagicMock()
        manager.resolve_template.side_effect = lambda value: value
        target = CloudWatch.__new__(CloudWatch)
        target.config_manager = manager
        target._interval_msecs = lambda: 5000

        def built(buffer_config):
            return json.loads(
                CloudWatch._build_streaming_config(target, buffer_config)
            )["streams"][0]["buffer"]

        # An integral float is honoured; a fractional one falls back rather than truncating.
        buffer = built({"maxDiskBytes": 4096.0, "segmentBytes": 1024.0})
        assert buffer["maxDiskBytes"] == 4096
        assert buffer["segmentBytes"] == 1024

        assert built({"maxDiskBytes": 4096.5})["maxDiskBytes"] == _DEFAULT_MAX_DISK_BYTES

        # A configured 0 is a real value, not an absent one.
        assert built({"maxDiskBytes": 0})["maxDiskBytes"] == 0

    def test_credentials_integers_accept_the_store_shape_and_reject_a_fractional(self, tmp_path):
        from edgecommons.credentials.config import _integer
        from edgecommons.credentials.errors import CredentialError

        assert _integer({"keepVersions": 3.0}, "keepVersions", 2, "p") == 3
        assert _integer({}, "keepVersions", 2, "p") == 2
        with pytest.raises(CredentialError) as excinfo:
            _integer({"keepVersions": 3.5}, "keepVersions", 2, "credentials.vault.keepVersions")
        assert str(excinfo.value) == (
            "configuration value 'credentials.vault.keepVersions' must be a whole number,"
            " but was 3.5"
        )
