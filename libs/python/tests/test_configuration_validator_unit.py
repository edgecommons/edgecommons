"""Unit tests for ggcommons.validation.configuration_validator.

Targets the previously-uncovered branches of ConfigurationValidator:
  * the module-level jsonschema-import guard (lines 17-18),
  * the schema-load fallbacks and failure paths in ``_load_schema`` (62-63, 77-99),
  * the fail-closed behaviour of ``validate`` when validation can't run (122-127),
  * the broad ``except`` in ``validate`` for non-ValidationError errors (154-155),
  * plus the happy path, the ValidationError path, ``validate_section`` and
    ``is_validation_available``.

All tests are hermetic: no network/AWS/broker, no real Greengrass paths. Schema
loading is exercised against the schema resource that ships inside the package.
"""

import importlib.resources as ir
import importlib.util
import sys

import pytest

from ggcommons.validation import configuration_validator as cv
from ggcommons.validation.configuration_validator import (
    ConfigurationValidator,
    ConfigurationValidationException,
)


@pytest.fixture(autouse=True)
def _reset_schema_cache():
    """Force a clean (re)load per test and restore the process-global cache after.

    ConfigurationValidator caches the schema on the class, so tests that toggle
    availability / patch loading must not leak that state into other tests.
    """
    orig_schema = ConfigurationValidator._schema
    orig_loaded = ConfigurationValidator._schema_loaded
    orig_available = cv.JSONSCHEMA_AVAILABLE

    ConfigurationValidator._schema = None
    ConfigurationValidator._schema_loaded = False
    try:
        yield
    finally:
        ConfigurationValidator._schema = orig_schema
        ConfigurationValidator._schema_loaded = orig_loaded
        cv.JSONSCHEMA_AVAILABLE = orig_available


def _raise(exc):
    """Return a callable that raises ``exc`` regardless of args (for monkeypatching)."""

    def _f(*args, **kwargs):
        raise exc

    return _f


class TestValidateHappyPath:
    def test_minimal_valid_config_passes(self):
        # Only `component` is required at the strict top level.
        assert ConfigurationValidator.validate({"component": {}}) is None

    def test_full_valid_config_passes(self):
        config = {
            "component": {"global": {"foo": "bar"}},
            "logging": {"level": "INFO"},
            "metricEmission": {"target": "log", "namespace": "ns"},
            "heartbeat": {"intervalSecs": 5, "measures": {"cpu": True}},
            "tags": {"env": "test"},
        }
        assert ConfigurationValidator.validate(config) is None

    def test_schema_is_cached_after_first_load(self):
        first = ConfigurationValidator._load_schema()
        second = ConfigurationValidator._load_schema()
        assert first is not None
        # Second call hits the `_schema_loaded` short-circuit and returns the same object.
        assert first is second


class TestValidateErrors:
    def test_none_config_raises_value_error(self):
        with pytest.raises(ValueError, match="cannot be None"):
            ConfigurationValidator.validate(None)

    def test_missing_required_component_raises(self):
        # required:["component"] at the root; absolute_path is empty for this error.
        with pytest.raises(ConfigurationValidationException) as ei:
            ConfigurationValidator.validate({})
        exc = ei.value
        assert "validation failed" in str(exc).lower()
        assert len(exc.validation_errors) == 1
        assert exc.validation_errors[0]["path"] == []

    def test_additional_property_at_strict_top_level_raises(self):
        # Top level is additionalProperties:false.
        with pytest.raises(ConfigurationValidationException):
            ConfigurationValidator.validate({"component": {}, "notAReal": 1})

    def test_wrong_type_includes_path_in_message(self):
        # `component` must be an object; a string trips a typed error with a path,
        # exercising the `e.absolute_path` branch of the error formatter.
        with pytest.raises(ConfigurationValidationException) as ei:
            ConfigurationValidator.validate({"component": "not-an-object"})
        exc = ei.value
        assert "at path: component" in str(exc)
        err = exc.validation_errors[0]
        assert err["path"] == ["component"]
        assert err["invalid_value"] == "not-an-object"
        assert err["schema_path"]

    def test_enum_violation_raises(self):
        with pytest.raises(ConfigurationValidationException):
            ConfigurationValidator.validate(
                {"component": {}, "logging": {"level": "LOUD"}}
            )

    def test_non_validation_error_wrapped(self, monkeypatch):
        # Force the jsonschema validate() call to raise a *non*-ValidationError, hitting
        # the broad `except Exception` branch (lines 154-155).
        monkeypatch.setattr(cv, "validate", _raise(RuntimeError("boom")))
        with pytest.raises(ConfigurationValidationException) as ei:
            ConfigurationValidator.validate({"component": {}})
        assert "validation error" in str(ei.value).lower()
        assert "boom" in str(ei.value)


class TestFailClosed:
    def test_fails_closed_when_jsonschema_unavailable(self, monkeypatch):
        # JSONSCHEMA_AVAILABLE False => _load_schema returns None (62-63) and
        # validate() raises with the "library is not installed" reason (122-124, 127).
        monkeypatch.setattr(cv, "JSONSCHEMA_AVAILABLE", False)
        assert ConfigurationValidator._load_schema() is None
        # reset so validate() re-evaluates the (still-disabled) load
        ConfigurationValidator._schema = None
        ConfigurationValidator._schema_loaded = False
        with pytest.raises(ConfigurationValidationException) as ei:
            ConfigurationValidator.validate({"component": {}})
        assert "jsonschema" in str(ei.value).lower()

    def test_fails_closed_when_schema_missing(self, monkeypatch):
        # jsonschema present but the packaged schema can't be found: the *else*
        # reason branch (line 125) and the raise (127).
        monkeypatch.setattr(
            ConfigurationValidator, "_load_schema", classmethod(lambda cls: None)
        )
        with pytest.raises(ConfigurationValidationException) as ei:
            ConfigurationValidator.validate({"component": {}})
        msg = str(ei.value).lower()
        assert "schema could not be found" in msg


class TestLoadSchemaFallbacks:
    def test_fallback_to_relative_path_when_resources_unavailable(self, monkeypatch):
        # Make the packaged-resources primary path raise (caught at line 77) so the
        # relative-path fallback (80-92) runs and loads the on-disk schema copy.
        monkeypatch.setattr(ir, "files", _raise(ModuleNotFoundError("no resources")))
        schema = ConfigurationValidator._load_schema()
        assert schema is not None
        assert "component" in schema.get("required", [])

    def test_load_returns_none_when_no_schema_found(self, monkeypatch):
        # Primary path raises (caught), and every fallback candidate "does not exist":
        # exercises the for-loop miss + "schema file not found" return (94-95).
        monkeypatch.setattr(ir, "files", _raise(ModuleNotFoundError("no resources")))
        monkeypatch.setattr(cv.Path, "exists", lambda self: False)
        assert ConfigurationValidator._load_schema() is None

    def test_unexpected_error_during_load_returns_none(self, monkeypatch):
        # A non-(Import/FileNotFound/ModuleNotFound) error from the primary path
        # propagates to the outer broad except and disables validation (97-99).
        monkeypatch.setattr(ir, "files", _raise(RuntimeError("kaboom")))
        assert ConfigurationValidator._load_schema() is None


class TestValidateSection:
    def test_none_section_raises_value_error(self):
        with pytest.raises(ValueError, match="cannot be None"):
            ConfigurationValidator.validate_section(None, "component")

    def test_empty_section_name_raises_value_error(self):
        with pytest.raises(ValueError, match="Section name"):
            ConfigurationValidator.validate_section({}, "")

    def test_valid_section_passes(self):
        # Wrapped as {"component": {}} which is valid.
        assert ConfigurationValidator.validate_section({}, "component") is None

    def test_invalid_section_reraised_with_context(self):
        # {"component": <str>} fails type validation; re-raised with section context.
        with pytest.raises(ConfigurationValidationException) as ei:
            ConfigurationValidator.validate_section("not-an-object", "component")
        exc = ei.value
        assert "section 'component'" in str(exc)
        # The original validation_errors are preserved through the re-raise.
        assert exc.validation_errors


class TestIsValidationAvailable:
    def test_available_when_jsonschema_and_schema_present(self):
        assert ConfigurationValidator.is_validation_available() is True

    def test_unavailable_when_schema_missing(self, monkeypatch):
        monkeypatch.setattr(
            ConfigurationValidator, "_load_schema", classmethod(lambda cls: None)
        )
        assert ConfigurationValidator.is_validation_available() is False


class TestExceptionType:
    def test_validation_errors_defaults_to_empty_list(self):
        exc = ConfigurationValidationException("msg")
        assert exc.validation_errors == []
        assert str(exc) == "msg"

    def test_validation_errors_passed_through(self):
        errs = [{"message": "x"}]
        exc = ConfigurationValidationException("msg", errs)
        assert exc.validation_errors is errs


class TestModuleImportGuard:
    def test_import_without_jsonschema_disables_validation(self, monkeypatch):
        """Re-exec the module source with `import jsonschema` failing.

        Setting sys.modules['jsonschema'] to None makes any `import jsonschema`
        raise ImportError, driving the module-level fallback (lines 17-18). The
        module is loaded under a throwaway name so the canonical, already-imported
        module (and the classes other test files hold) are left untouched.
        """
        monkeypatch.setitem(sys.modules, "jsonschema", None)
        spec = importlib.util.spec_from_file_location(
            "ggcommons_cfgvalidator_nojsonschema_probe", cv.__file__
        )
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        assert module.JSONSCHEMA_AVAILABLE is False
        # The module still defines its public surface even without jsonschema.
        assert hasattr(module, "ConfigurationValidator")
        assert hasattr(module, "ConfigurationValidationException")
