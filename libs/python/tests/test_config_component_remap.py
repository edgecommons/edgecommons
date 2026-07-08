"""Unit tests for the CONFIG_COMPONENT Flow-A remap onto UNS topics
(UNS-CANONICAL-DESIGN §4.3, D-U19): the get-configuration rendezvous, the self-ID
bootstrap body, the set-config inbox, and the 3-attempt fresh-request retry under the
framework request deadline (§5)."""
import json
from types import SimpleNamespace

import pytest

import edgecommons.config.manager.config_component_manager as ccm
from edgecommons.messaging.errors import RequestTimeoutError
from edgecommons.messaging.message import Message


@pytest.fixture
def no_subscribe(monkeypatch):
    subscribed = {}
    monkeypatch.setattr(
        ccm.MessagingClient, "subscribe",
        staticmethod(lambda topic, cb: subscribed.update(topic=topic, cb=cb)),
    )
    return subscribed


def _reply(body):
    reply = Message()
    reply.body = body
    return reply


def _bundle(component, component_config=None):
    if component_config is None:
        component_config = {"component": {}}
    return {
        "lineageVersion": 1,
        "catalogVersion": "test-catalog",
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


class TestFlowATopics:
    def test_topics_and_bootstrap_body(self, no_subscribe, monkeypatch):
        requests = []

        def fake_request(topic, msg):
            requests.append((topic, msg))
            return SimpleNamespace(get=lambda timeout=None: (True, _reply(_bundle("My_Comp"))))

        monkeypatch.setattr(ccm.MessagingClient, "request", staticmethod(fake_request))
        mgr = ccm.ConfigComponentManager("thing 1", "com.example.My+Comp")

        topic, msg = requests[0]
        # tokens minted locally through the normative sanitizer (§1.5 steps 4-5)
        assert topic == "ecv1/thing 1/config/main/cmd/get-configuration"
        assert no_subscribe["topic"] == "ecv1/thing 1/My_Comp/main/cmd/set-config"
        # the bootstrap request self-identifies in the BODY and carries no identity
        # (built without a config-bound builder)
        assert msg.get_body() == {"component": "My_Comp"}
        assert msg.get_identity() is None
        assert msg.get_tags() is None
        assert "get:" in mgr.get_config_source()

    def test_set_config_push_applies_config(self, no_subscribe, monkeypatch):
        monkeypatch.setattr(
            ccm.MessagingClient, "request",
            staticmethod(lambda t, m: SimpleNamespace(
                get=lambda timeout=None: (True, _reply(_bundle("C"))))),
        )
        mgr = ccm.ConfigComponentManager("thing-1", "com.example.C")
        mgr.complete_initialization()

        push = Message()
        push.body = _bundle("C", {"component": {"global": {"k": "pushed"}}})
        no_subscribe["cb"](no_subscribe["topic"], push)
        assert mgr.get_global_config() == {"k": "pushed"}


class TestRetryPolicy:
    def test_fresh_request_per_attempt_then_success(self, no_subscribe, monkeypatch):
        """The framework deadline settles the request, so a retry must issue a FRESH
        request — waiting on the settled Iou can never succeed (§5)."""
        attempts = []

        class _TimedOutIou:
            def get(self, timeout=None):
                raise RequestTimeoutError("expired")

        def fake_request(topic, msg):
            attempts.append(msg)
            if len(attempts) < 3:
                return _TimedOutIou()
            return SimpleNamespace(get=lambda timeout=None: (
                True,
                _reply(_bundle("C", {"component": {"global": {"k": "third-time"}}})),
            ))

        monkeypatch.setattr(ccm.MessagingClient, "request", staticmethod(fake_request))
        mgr = ccm.ConfigComponentManager("thing-1", "com.example.C")
        assert mgr.get_global_config() == {"k": "third-time"}
        assert len(attempts) == 3
        # every attempt was a distinct, fresh request message
        assert len({id(m) for m in attempts}) == 3

    def test_three_deadline_timeouts_raise(self, no_subscribe, monkeypatch):
        class _TimedOutIou:
            def get(self, timeout=None):
                raise RequestTimeoutError("expired")

        monkeypatch.setattr(
            ccm.MessagingClient, "request", staticmethod(lambda t, m: _TimedOutIou())
        )
        with pytest.raises(RuntimeError, match="after 3 tries"):
            ccm.ConfigComponentManager("thing-1", "com.example.C")

    def test_get_expiry_with_disabled_deadline_cancels_and_retries(self, no_subscribe, monkeypatch):
        """When the framework deadline is disabled, get() expires with (False, iou):
        the abandoned request must be canceled (settle + cleanup) before re-issuing."""
        canceled = []
        pending = []

        class _NeverIou:
            def get(self, timeout=None):
                return (False, self)

        def fake_request(topic, msg):
            iou = _NeverIou()
            pending.append(iou)
            return iou

        monkeypatch.setattr(ccm.MessagingClient, "request", staticmethod(fake_request))
        monkeypatch.setattr(
            ccm.MessagingClient, "cancel_request", staticmethod(lambda iou: canceled.append(iou))
        )
        with pytest.raises(RuntimeError, match="after 3 tries"):
            ccm.ConfigComponentManager("thing-1", "com.example.C")
        assert len(pending) == 3
        assert canceled == pending  # each abandoned request was canceled

    def test_str_reply_body_parsed_as_json(self, no_subscribe, monkeypatch):
        monkeypatch.setattr(
            ccm.MessagingClient, "request",
            staticmethod(lambda t, m: SimpleNamespace(get=lambda timeout=None: (
                True,
                _reply(json.dumps(_bundle("C", {"component": {"global": {"k": "str"}}}))),
            ))),
        )
        mgr = ccm.ConfigComponentManager("thing-1", "com.example.C")
        assert mgr.get_global_config() == {"k": "str"}
