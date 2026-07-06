"""Unit tests for the ConfigManager UNS additions (UNS-CANONICAL-DESIGN §1.5, D-U1/
D-U2/D-U10/D-U25): once-at-init component-identity resolution from the component's
OWN config, the topic.includeRoot flag, and messaging.requestTimeoutSeconds."""
import pytest

from edgecommons.config.manager.config_manager import ConfigManager


class _Manager(ConfigManager):
    """A ConfigManager whose _load_configuration returns a canned dict."""

    def __init__(self, config, component="com.example.opcua-adapter", thing="gw-01"):
        super().__init__(component, thing, validate_config=False)
        self._canned = config
        self.init()

    def _load_configuration(self):
        return self._canned


class TestZeroConfigDefault:
    def test_default_hierarchy_is_device_only(self):
        m = _Manager({"component": {}})
        ident = m.get_component_identity()
        assert [e.level for e in ident.hier] == ["device"]
        assert ident.device == "gw-01"
        assert ident.path == "gw-01"
        assert ident.component == "opcua-adapter"  # sanitized short name (D-U18)
        assert ident.instance == "main"

    def test_identity_none_without_init(self):
        cm = ConfigManager("comp", "thing")
        assert cm.get_component_identity() is None


class TestMultiLevelResolution:
    CONFIG = {
        "component": {},
        "hierarchy": {"levels": ["site", "zone", "device"]},
        "identity": {"site": "dallas", "zone": "z-3"},
    }

    def test_values_from_identity_config_last_is_thing(self):
        m = _Manager(self.CONFIG)
        ident = m.get_component_identity()
        assert [(e.level, e.value) for e in ident.hier] == [
            ("site", "dallas"), ("zone", "z-3"), ("device", "gw-01"),
        ]
        assert ident.path == "dallas/z-3/gw-01"

    def test_values_are_sanitized_with_warn(self):
        cfg = {
            "component": {},
            "hierarchy": {"levels": ["site", "device"]},
            "identity": {"site": "dal/las"},
        }
        m = _Manager(cfg, thing="gw+01")
        ident = m.get_component_identity()
        assert ident.hier[0].value == "dal_las"
        assert ident.device == "gw_01"

    def test_component_short_name_sanitized(self):
        m = _Manager({"component": {}}, component="com.example.opcua+adapter")
        assert m.get_component_identity().component == "opcua_adapter"

    def test_component_token_overrides_pascal_component_name(self):
        m = _Manager(
            {"component": {"token": "opcua-adapter"}},
            component="com.mbreissi.edgecommons.OpcUaAdapter",
        )
        assert m.get_component_identity().component == "opcua-adapter"


class TestFailFast:
    def test_missing_identity_value_names_the_level(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["site", "zone", "device"]},
               "identity": {"site": "dallas"}}
        with pytest.raises(ValueError, match="zone"):
            _Manager(cfg)

    def test_device_level_key_is_an_error(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["site", "device"]},
               "identity": {"site": "s", "device": "forged"}}
        with pytest.raises(ValueError, match="device"):
            _Manager(cfg)

    def test_undeclared_identity_key_is_an_error(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["site", "device"]},
               "identity": {"site": "s", "typo": "x"}}
        with pytest.raises(ValueError, match="typo"):
            _Manager(cfg)

    def test_bad_level_name_rejected(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["si te", "device"]},
               "identity": {"si te": "s"}}
        with pytest.raises(ValueError, match="level name"):
            _Manager(cfg)

    def test_duplicate_level_rejected(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["site", "site"]},
               "identity": {"site": "s"}}
        with pytest.raises(ValueError, match="duplicate"):
            _Manager(cfg)

    def test_empty_levels_rejected(self):
        cfg = {"component": {}, "hierarchy": {"levels": []}}
        with pytest.raises(ValueError, match="non-empty"):
            _Manager(cfg)

    def test_non_string_level_rejected(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["site", 42]}}
        with pytest.raises(ValueError, match="strings"):
            _Manager(cfg)

    def test_malformed_hierarchy_section_rejected(self):
        with pytest.raises(ValueError, match="hierarchy"):
            _Manager({"component": {}, "hierarchy": "nope"})

    def test_malformed_identity_section_rejected(self):
        with pytest.raises(ValueError, match="identity"):
            _Manager({"component": {}, "hierarchy": {"levels": ["a", "device"]},
                      "identity": "nope"})

    def test_malformed_component_token_rejected(self):
        with pytest.raises(ValueError, match="component.token"):
            _Manager({"component": {"token": ""}})

    def test_missing_thing_name_rejected(self):
        with pytest.raises(ValueError, match="thing name"):
            _Manager({"component": {}}, thing=None)


class TestTopicIncludeRoot:
    def test_default_false(self):
        assert _Manager({"component": {}}).is_topic_include_root() is False

    def test_parsed_true(self):
        cfg = {"component": {}, "topic": {"includeRoot": True},
               "hierarchy": {"levels": ["site", "device"]}, "identity": {"site": "s"}}
        assert _Manager(cfg).is_topic_include_root() is True

    def test_lenient_on_malformed(self):
        assert _Manager({"component": {}, "topic": "x"}).is_topic_include_root() is False

    def test_single_level_include_root_warns_once(self, caplog):
        # D-U25: includeRoot with the single-level default is a no-op + a config WARN.
        import logging
        with caplog.at_level(logging.WARNING):
            m = _Manager({"component": {}, "topic": {"includeRoot": True}})
        warnings = [r for r in caplog.records if "includeRoot" in r.getMessage()]
        assert len(warnings) == 1
        assert m.is_topic_include_root() is True  # the raw flag is still reported


class TestRequestTimeoutSeconds:
    def test_default_30(self):
        assert _Manager({"component": {}}).get_messaging_request_timeout() == 30.0

    def test_configured_value(self):
        cfg = {"component": {}, "messaging": {"requestTimeoutSeconds": 12.5}}
        assert _Manager(cfg).get_messaging_request_timeout() == 12.5

    def test_zero_disables(self):
        cfg = {"component": {}, "messaging": {"requestTimeoutSeconds": 0}}
        assert _Manager(cfg).get_messaging_request_timeout() == 0.0

    def test_lenient_on_malformed(self):
        assert _Manager(
            {"component": {}, "messaging": {"requestTimeoutSeconds": "soon"}}
        ).get_messaging_request_timeout() == 30.0
        assert _Manager(
            {"component": {}, "messaging": {"requestTimeoutSeconds": -5}}
        ).get_messaging_request_timeout() == 30.0
        assert _Manager(
            {"component": {}, "messaging": {"requestTimeoutSeconds": True}}
        ).get_messaging_request_timeout() == 30.0
        assert _Manager({"component": {}, "messaging": "x"}).get_messaging_request_timeout() == 30.0


class TestEffectiveConfigRetention:
    def test_raw_config_retained_verbatim(self):
        cfg = {"component": {}, "hierarchy": {"levels": ["device"]},
               "messaging": {"requestTimeoutSeconds": 5}}
        m = _Manager(cfg)
        assert m.get_effective_config() is cfg

    def test_hot_reload_refreshes_flags_not_identity(self):
        m = _Manager({"component": {}})
        ident = m.get_component_identity()
        m._apply_config({"component": {}, "topic": {"includeRoot": True},
                         "messaging": {"requestTimeoutSeconds": 3}})
        assert m.is_topic_include_root() is True
        assert m.get_messaging_request_timeout() == 3.0
        # identity is resolved ONCE at init (§1.5), not re-resolved on reload
        assert m.get_component_identity() is ident

    def test_sanitize_is_public_static(self):
        # D-U26: the sanitizer is the normative UNS channel-token sanitizer.
        assert ConfigManager.sanitize("a+b/c\\d#e") == "a_b_c_d_e"
        assert ConfigManager.sanitize("a..b") == "a_b"
        assert ConfigManager.sanitize("gw\x8501") == "gw_01"  # C1 control
        assert ConfigManager.sanitize(None) == ""
