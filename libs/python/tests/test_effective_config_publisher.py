"""Unit tests for the library-owned ``cfg`` publisher (UNS-CANONICAL-DESIGN §4.3):
the redaction walk, the UNS topic, the privileged-seam publish, the config-change
re-announce, and the best-effort failure handling."""
from ggcommons.config.effective_config_publisher import (
    EffectiveConfigPublisher,
    REDACTED,
    redact,
)
from ggcommons.messaging.identity import HierEntry, MessageIdentity


class _FakeMessaging:
    def __init__(self):
        self.reserved = []

    def _publish_reserved(self, topic, msg):
        self.reserved.append((topic, msg))


class _FakeConfigManager:
    def __init__(self, effective=None, identity=True, include_root=False):
        self._effective = effective if effective is not None else {"component": {"global": {}}}
        self._identity = (
            MessageIdentity([HierEntry("device", "gw-01")], "opcua-adapter")
            if identity else None
        )
        self._include_root = include_root
        self.listeners = []

    def get_component_identity(self):
        return self._identity

    def is_topic_include_root(self):
        return self._include_root

    def get_effective_config(self):
        return self._effective

    def get_tag_config(self):
        return None

    def add_config_change_listener(self, listener):
        self.listeners.append(listener)


class TestRedaction:
    def test_password_and_pin_redacted_anywhere(self):
        cfg = {
            "component": {"global": {"db": {"password": "s3cret", "PIN": "1234"}}},
            "password": "top",
        }
        red = redact(cfg)
        assert red["component"]["global"]["db"]["password"] == REDACTED
        assert red["component"]["global"]["db"]["PIN"] == REDACTED
        assert red["password"] == REDACTED
        # original not mutated
        assert cfg["component"]["global"]["db"]["password"] == "s3cret"

    def test_credentials_redacted_only_under_top_level_messaging(self):
        cfg = {
            "messaging": {"local": {"credentials": {"username": "u", "password": "p"}}},
            "component": {"global": {"credentials": {"apiKey": "k"},
                                     "messaging": {"credentials": "nested"}}},
        }
        red = redact(cfg)
        assert red["messaging"]["local"]["credentials"] == REDACTED
        # a credentials key OUTSIDE the top-level messaging section is kept...
        assert red["component"]["global"]["credentials"] == {"apiKey": "k"}
        # ...and a nested `messaging` key elsewhere does not trigger the rule
        assert red["component"]["global"]["messaging"]["credentials"] == "nested"

    def test_secret_refs_untouched(self):
        cfg = {"streaming": {"kinesis": {"accessKey": {"$secret": "aws/key"}}}}
        assert redact(cfg) == cfg  # $secret refs are never resolved here

    def test_dicts_inside_lists_are_walked(self):
        cfg = {"component": {"instances": [{"id": "a", "password": "x"}]}}
        assert redact(cfg)["component"]["instances"][0]["password"] == REDACTED


class TestPublish:
    def test_publish_now_announces_on_cfg_topic(self):
        cm = _FakeConfigManager({"component": {"name": "x"},
                                 "messaging": {"local": {"credentials": "c"}}})
        messaging = _FakeMessaging()
        pub = EffectiveConfigPublisher(cm, messaging)
        pub.publish_now()
        assert len(messaging.reserved) == 1
        topic, msg = messaging.reserved[0]
        assert topic == "ecv1/gw-01/opcua-adapter/main/cfg"
        assert msg.get_header().name == "cfg"
        assert msg.get_header().version == "1.0"
        body = msg.get_body()
        assert body["config"]["component"] == {"name": "x"}
        assert body["config"]["messaging"]["local"]["credentials"] == REDACTED
        # config-bound builder stamps the identity
        assert msg.get_identity().component == "opcua-adapter"

    def test_registered_as_config_change_listener(self):
        cm = _FakeConfigManager()
        messaging = _FakeMessaging()
        pub = EffectiveConfigPublisher(cm, messaging)
        assert pub in cm.listeners
        assert pub.on_configuration_change({}) is True
        assert len(messaging.reserved) == 1  # the change re-announced

    def test_no_identity_warns_once_and_skips(self):
        cm = _FakeConfigManager(identity=False)
        messaging = _FakeMessaging()
        pub = EffectiveConfigPublisher(cm, messaging)
        pub.publish_now()
        pub.publish_now()
        assert messaging.reserved == []

    def test_no_effective_config_skips(self):
        cm = _FakeConfigManager()
        cm._effective = None
        messaging = _FakeMessaging()
        EffectiveConfigPublisher(cm, messaging).publish_now()
        assert messaging.reserved == []

    def test_publish_failure_is_swallowed(self):
        cm = _FakeConfigManager()

        class _Boom:
            def _publish_reserved(self, topic, msg):
                raise RuntimeError("broker down")

        EffectiveConfigPublisher(cm, _Boom()).publish_now()  # no exception

    def test_none_args_rejected(self):
        import pytest
        with pytest.raises(ValueError):
            EffectiveConfigPublisher(None, _FakeMessaging())
        with pytest.raises(ValueError):
            EffectiveConfigPublisher(_FakeConfigManager(), None)
