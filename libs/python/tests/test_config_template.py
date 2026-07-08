"""
Tests for ConfigManager.resolve_template, focusing on value sanitization
(parity with the Java/Rust libraries: substituted values are neutralized so they
cannot inject path traversal or MQTT topic wildcards, while the template's own
separators are preserved).
"""

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.tag_config import TagConfiguration
from edgecommons.messaging.identity import HierEntry, MessageIdentity


def _manager(tags=None, identity=None):
    cm = ConfigManager("com.test.TestComponent", "test-thing")
    cm._tag_config = TagConfiguration(tags or {})
    cm._component_identity = identity
    return cm


def test_resolve_template_substitutes_builtins_and_tags():
    cm = _manager({"environment": "production"})
    resolved = cm.resolve_template(
        "/var/log/{ComponentName}-{ThingName}-{environment}.log"
    )
    assert resolved == "/var/log/TestComponent-test-thing-production.log"


def test_resolve_template_component_full_vs_short_name():
    cm = _manager()
    assert cm.resolve_template("{ComponentName}") == "TestComponent"
    assert cm.resolve_template("{ComponentFullName}") == "com.test.TestComponent"


def test_resolve_template_leaves_unknown_placeholder_untouched():
    cm = _manager()
    assert cm.resolve_template("{Unknown}") == "{Unknown}"


def test_resolve_template_sanitizes_hostile_values():
    # Path separators, traversal dots, and MQTT wildcards in a substituted value
    # are each replaced with '_'; the template's own '/' separators are preserved.
    cm = _manager({"evil": "a/b\\c+d#e..g"})
    resolved = cm.resolve_template("prefix/{evil}/suffix")
    assert resolved == "prefix/a_b_c_d_e_g/suffix"
    assert "{evil}" not in resolved


def test_resolve_template_substitutes_identity_without_tag_duplication():
    identity = MessageIdentity(
        [HierEntry("site", "plant-1"), HierEntry("device", "gw-01")],
        "TestComponent",
    )
    cm = _manager(identity=identity)

    assert (
        cm.resolve_template("logs/{site}/{device}/{line}")
        == "logs/plant-1/gw-01/{line}"
    )


def test_resolve_template_sanitizes_identity_values_like_tags():
    identity = MessageIdentity(
        [HierEntry("site", "plant/a+b#c..d"), HierEntry("device", "gw\\01")],
        "TestComponent",
    )
    cm = _manager(identity=identity)

    assert cm.resolve_template("{site}/{device}") == "plant_a_b_c_d/gw_01"


def test_resolve_template_identity_wins_over_colliding_tag_key():
    identity = MessageIdentity(
        [HierEntry("site", "identity-site"), HierEntry("device", "gw-01")],
        "TestComponent",
    )
    cm = _manager({"site": "tag-site"}, identity=identity)

    assert cm.resolve_template("{site}") == "identity-site"


def test_resolve_template_builtins_win_over_identity_and_tags():
    identity = MessageIdentity(
        [
            HierEntry("ThingName", "identity-thing"),
            HierEntry("ComponentName", "identity-component"),
            HierEntry("device", "gw-01"),
        ],
        "TestComponent",
    )
    cm = _manager(
        {"ThingName": "tag-thing", "ComponentName": "tag-component"},
        identity=identity,
    )

    assert (
        cm.resolve_template("{ThingName}/{ComponentName}")
        == "test-thing/TestComponent"
    )


def test_resolve_template_preserves_clean_dotted_names():
    # Single dots (reverse-DNS component name) are not a traversal sequence and
    # must survive sanitization intact.
    cm = _manager()
    assert (
        cm.resolve_template("/var/log/{ComponentFullName}.log")
        == "/var/log/com.test.TestComponent.log"
    )
