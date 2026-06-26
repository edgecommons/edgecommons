"""Unit tests for builder validation: MetricBuilder, GGCommonsBuilder, and the
MessagingProvider.topic_matches_sub wildcard matcher.
"""
import pytest

from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.metrics.metric import Metric
from ggcommons.ggcommons_builder import GGCommonsBuilder
from ggcommons.messaging.messaging_provider import MessagingProvider


class TestMetricBuilderValidation:
    def test_create_empty_name_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("")

    def test_namespace_empty_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").with_namespace("")

    def test_thing_name_empty_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").with_thing_name("")

    def test_component_name_empty_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").with_component_name("")

    def test_measure_name_empty_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").add_measure("", "Count")

    def test_measure_unit_empty_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").add_measure("x", "")

    def test_measure_bad_storage_resolution_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").add_measure("x", "Count", 30)

    def test_dimension_empty_key_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").add_dimension("", "v")

    def test_dimension_empty_value_raises(self):
        with pytest.raises(ValueError):
            MetricBuilder.create("m").add_dimension("k", "")

    def test_too_many_dimensions_raises(self):
        builder = MetricBuilder.create("m").with_thing_name("t").with_component_name("c")
        # 8 user dims + 3 default (coreName/category/component) = 11 > MAX_DIMENSIONS (10)
        for i in range(8):
            builder.add_dimension(f"d{i}", "v")
        with pytest.raises(ValueError, match="at most"):
            builder.build()

    def test_with_config_sets_names(self):
        class CfgSvc:
            def get_thing_name(self):
                return "thing-x"

            def get_component_name(self):
                return "comp-x"

        m = (
            MetricBuilder.create("m")
            .with_config(CfgSvc())
            .add_measure("v", "Count", 60)
            .build()
        )
        assert m.get_dimensions()["coreName"] == "thing-x"
        assert m.get_dimensions()["component"] == "comp-x"


class TestGGCommonsBuilderValidation:
    def test_create_empty_raises(self):
        with pytest.raises(ValueError):
            GGCommonsBuilder.create("")

    def test_with_args_none_raises(self):
        with pytest.raises(ValueError):
            GGCommonsBuilder.create("com.example.C").with_args(None)

    def test_with_app_options_none_raises(self):
        with pytest.raises(ValueError):
            GGCommonsBuilder.create("com.example.C").with_app_options(None)

    def test_chaining_and_build(self, monkeypatch):
        import argparse
        import ggcommons

        captured = {}

        class FakeGGCommons:
            def __init__(self, component_name, args, app_options, receive_own_messages):
                captured["component_name"] = component_name
                captured["args"] = args
                captured["app_options"] = app_options
                captured["receive_own_messages"] = receive_own_messages

        monkeypatch.setattr(ggcommons, "GGCommons", FakeGGCommons)

        parser = argparse.ArgumentParser()
        result = (
            GGCommonsBuilder.create("com.example.C")
            .with_args(["-c", "FILE", "x.json"])
            .with_app_options(parser)
            .receive_own_messages(False)
            .build()
        )
        assert isinstance(result, FakeGGCommons)
        assert captured["component_name"] == "com.example.C"
        assert captured["args"] == ["-c", "FILE", "x.json"]
        assert captured["app_options"] is parser
        assert captured["receive_own_messages"] is False

    def test_build_defaults_to_empty_args(self, monkeypatch):
        import ggcommons

        captured = {}

        class FakeGGCommons:
            def __init__(self, component_name, args, app_options, receive_own_messages):
                captured["args"] = args
                captured["receive_own_messages"] = receive_own_messages

        monkeypatch.setattr(ggcommons, "GGCommons", FakeGGCommons)
        GGCommonsBuilder.create("com.example.C").build()
        assert captured["args"] == []
        # default receive_own_messages is True
        assert captured["receive_own_messages"] is True


class TestTopicMatchesSub:
    def test_exact_match(self):
        assert MessagingProvider.topic_matches_sub("a/b/c", "a/b/c") is True

    def test_no_match(self):
        assert MessagingProvider.topic_matches_sub("a/b/c", "a/b/d") is False

    def test_single_level_wildcard(self):
        assert MessagingProvider.topic_matches_sub("a/+/c", "a/b/c") is True
        assert MessagingProvider.topic_matches_sub("a/+/c", "a/b/x/c") is False

    def test_single_level_wildcard_trailing(self):
        assert MessagingProvider.topic_matches_sub("a/+", "a/b") is True

    def test_multilevel_wildcard(self):
        assert MessagingProvider.topic_matches_sub("a/#", "a/b/c/d") is True
        assert MessagingProvider.topic_matches_sub("a/#", "a") is True

    def test_parent_matches_hash(self):
        # foo matches foo/#
        assert MessagingProvider.topic_matches_sub("foo/#", "foo") is True

    def test_hash_not_terminal_is_false(self):
        # '#' must be the last char of the sub
        assert MessagingProvider.topic_matches_sub("a/#/b", "a/x/b") is False

    def test_dollar_topic_mismatch(self):
        # a '$'-topic must only match a '$'-subscription
        assert MessagingProvider.topic_matches_sub("a/b", "$SYS/x") is False
        assert MessagingProvider.topic_matches_sub("$SYS/#", "a/b") is False

    def test_dollar_topic_match(self):
        assert MessagingProvider.topic_matches_sub("$SYS/#", "$SYS/broker/uptime") is True
