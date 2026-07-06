"""Unit tests for the reserved-class publish guard + the privileged internal-publish
seam (UNS-CANONICAL-DESIGN §4.1/§4.2, D-U4/D-U8/D-U24/D-U27), driven against a fake
provider on the static MessagingClient (no broker)."""
import pytest

from edgecommons.messaging.errors import ReservedTopicError
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_client import MessagingClient


class _FakeProvider:
    def __init__(self):
        self.published = []
        self.published_raw = []
        self.published_iot = []
        self.published_iot_raw = []
        self.requests = []
        self.replies = []

    def publish(self, topic, msg):
        self.published.append((topic, msg))

    def publish_raw(self, topic, msg):
        self.published_raw.append((topic, msg))

    def publish_to_iot_core(self, topic, msg, qos):
        self.published_iot.append((topic, msg, qos))

    def publish_to_iot_core_raw(self, topic, msg, qos):
        self.published_iot_raw.append((topic, msg, qos))

    def request(self, topic, msg, timeout_secs=None):
        self.requests.append((topic, msg, timeout_secs))
        return "iou"

    def request_from_iot_core(self, topic, msg, timeout_secs=None):
        self.requests.append((topic, msg, timeout_secs))
        return "iou"

    def reply(self, request, reply):
        self.replies.append((request, reply))

    def reply_to_iot_core(self, request, reply):
        self.replies.append((request, reply))

    def set_default_request_timeout(self, secs):
        self.default_timeout = secs

    def get_default_request_timeout(self):
        return getattr(self, "default_timeout", 30.0)

    def disconnect(self):
        pass


@pytest.fixture
def provider():
    fake = _FakeProvider()
    MessagingClient._messaging_provider = fake
    MessagingClient._guard_include_root = False
    yield fake
    MessagingClient._messaging_provider = None
    MessagingClient._guard_include_root = False


def _msg(reply_to=None):
    b = MessageBuilder.create("N", "1").with_payload({"v": 1})
    if reply_to:
        b = b.with_reply_to(reply_to)
    return b.build()


class TestGuardPredicate:
    @pytest.mark.parametrize("topic,include_root,reserved", [
        # position 4 (rootless grammar), always checked
        ("ecv1/gw/comp/main/state", False, "state"),
        ("ecv1/gw/comp/main/metric/cpu", False, "metric"),
        ("ecv1/gw/comp/main/cfg", False, "cfg"),
        ("ecv1/gw/comp/main/log/tail", False, "log"),
        ("ecv1/gw/comp/main/cfg", True, "cfg"),
        # position 5 only when the effective root mode is on (D-U24/D-U27)
        ("ecv1/site/gw/comp/main/state", True, "state"),
        ("ecv1/site/gw/comp/main/state", False, None),
        # legit app channels whose first token is a reserved word
        ("ecv1/gw/comp/main/app/state", False, None),
        ("ecv1/site/gw/comp/main/app/state", True, None),
        # non-reserved classes pass
        ("ecv1/gw/comp/main/data/temp", False, None),
        ("ecv1/gw/comp/main/cmd/set-config", False, None),
        # non-ecv1 topics are structurally exempt (D-U6/D-U21)
        ("edgecommons/reply-42", False, None),
        ("cloudwatch/metric/put", False, None),
        ("ecv1x/gw/comp/main/state", False, None),  # prefix but different token
        # short topics pass (no class position)
        ("ecv1/gw/state", False, None),
        (None, False, None),
        ("", False, None),
    ])
    def test_reserved_class_of(self, topic, include_root, reserved):
        assert MessagingClient._reserved_class_of(topic, include_root) == reserved


class TestGuardedMethods:
    def test_publish_rejects_reserved(self, provider):
        with pytest.raises(ReservedTopicError) as e:
            MessagingClient.publish("ecv1/gw/comp/main/state", _msg())
        assert e.value.topic == "ecv1/gw/comp/main/state"
        assert e.value.class_token == "state"
        assert provider.published == []

    def test_publish_raw_rejects_reserved(self, provider):
        with pytest.raises(ReservedTopicError):
            MessagingClient.publish_raw("ecv1/gw/comp/main/metric/cpu", {"v": 1})
        assert provider.published_raw == []

    def test_publish_to_iot_core_rejects_reserved(self, provider):
        with pytest.raises(ReservedTopicError):
            MessagingClient.publish_to_iot_core("ecv1/gw/comp/main/cfg", _msg(), 1)
        assert provider.published_iot == []

    def test_publish_to_iot_core_raw_rejects_reserved(self, provider):
        with pytest.raises(ReservedTopicError):
            MessagingClient.publish_to_iot_core_raw("ecv1/gw/comp/main/log/x", {}, 1)
        assert provider.published_iot_raw == []

    def test_request_rejects_reserved(self, provider):
        with pytest.raises(ReservedTopicError):
            MessagingClient.request("ecv1/gw/comp/main/state", _msg())
        assert provider.requests == []

    def test_request_from_iot_core_rejects_reserved(self, provider):
        with pytest.raises(ReservedTopicError):
            MessagingClient.request_from_iot_core("ecv1/gw/comp/main/state", _msg())
        assert provider.requests == []

    def test_reply_guards_hostile_reply_to(self, provider):
        # D-U8: a hostile requester could set header.reply_to to a victim's reserved
        # topic and turn an innocent responder into a forger.
        request = _msg(reply_to="ecv1/gw/victim/main/state")
        with pytest.raises(ReservedTopicError):
            MessagingClient.reply(request, _msg())
        assert provider.replies == []

    def test_reply_to_iot_core_guards_hostile_reply_to(self, provider):
        request = _msg(reply_to="ecv1/gw/victim/main/cfg")
        with pytest.raises(ReservedTopicError):
            MessagingClient.reply_to_iot_core(request, _msg())
        assert provider.replies == []

    def test_reply_without_header_passes_to_provider(self, provider):
        from edgecommons.messaging.message import Message
        MessagingClient.reply(Message(), _msg())
        assert len(provider.replies) == 1

    def test_allowed_topics_pass(self, provider):
        MessagingClient.publish("ecv1/gw/comp/main/data/temp", _msg())
        MessagingClient.publish("edgecommons/reply-42", _msg())
        MessagingClient.publish_raw("cloudwatch/metric/put", {"v": 1})
        assert len(provider.published) == 2
        assert len(provider.published_raw) == 1

    def test_include_root_binding_enables_position5(self, provider):
        rooted = "ecv1/site/gw/comp/main/state"
        MessagingClient.publish(rooted, _msg())  # rootless guard: passes
        MessagingClient.set_guard_include_root(True)
        with pytest.raises(ReservedTopicError):
            MessagingClient.publish(rooted, _msg())

    def test_shutdown_resets_guard_flag(self, provider):
        MessagingClient.set_guard_include_root(True)
        MessagingClient.shutdown()
        assert MessagingClient._guard_include_root is False


class TestPrivilegedSeam:
    def test_publish_reserved_bypasses_guard(self, provider):
        MessagingClient._publish_reserved("ecv1/gw/comp/main/state", _msg())
        assert len(provider.published) == 1

    def test_publish_reserved_raw_bypasses_guard(self, provider):
        MessagingClient._publish_reserved_raw("ecv1/gw/comp/main/metric/cpu", {"v": 1})
        assert len(provider.published_raw) == 1

    def test_publish_reserved_to_iot_core_bypasses_guard(self, provider):
        MessagingClient._publish_reserved_to_iot_core("ecv1/gw/comp/main/cfg", _msg(), 1)
        assert len(provider.published_iot) == 1


class TestDeadlineBinding:
    def test_default_request_timeout_binding(self, provider):
        MessagingClient.set_default_request_timeout(12.5)
        assert provider.default_timeout == 12.5
        assert MessagingClient.get_default_request_timeout() == 12.5

    def test_binding_with_no_provider_is_noop(self, provider):
        MessagingClient._messaging_provider = None
        MessagingClient.set_default_request_timeout(5)  # no raise
        assert MessagingClient.get_default_request_timeout() is None

    def test_request_passes_per_call_timeout(self, provider):
        MessagingClient.request("some/topic", _msg(), 7.5)
        assert provider.requests[-1][2] == 7.5
