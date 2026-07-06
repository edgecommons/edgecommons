"""Unit tests for the dual-MQTT StandaloneProvider using a MOCK paho client.

These exercise the full provider lifecycle (client creation, connect, callbacks,
subscribe/SUBACK, dispatch, request/reply, unsubscribe, disconnect) WITHOUT a real
broker by replacing ``paho.mqtt.client.Client`` with an in-process fake. The fake
simulates an async SUBACK and lets tests inject inbound messages, so the blocking
subscription path and the executor dispatch run for real.

The live dual-broker / TLS paths are covered by the integration suite
(test_dual_broker_integration.py) and are out of scope here.
"""
import json
import threading
import time
from types import SimpleNamespace

import paho.mqtt.client as mqtt
import pytest

import edgecommons.messaging.providers.standalone_provider as sp
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider, _BrokerChannel
from edgecommons.messaging.messaging_config import (
    MessagingConfiguration,
    MessagingConfigData,
    LocalMqttConfig,
    NorthboundMqttConfig,
    CredentialsConfig,
)
from edgecommons.messaging.message import Message, MessageHeader
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.qos import Qos


# --------------------------------------------------------------------------- fakes


class FakeMqttClient:
    """In-process stand-in for ``paho.mqtt.client.Client``."""

    instances = []

    def __init__(self, callback_api_version=None, client_id=None):
        self.callback_api_version = callback_api_version
        self.client_id = client_id
        self._connected = False
        self.on_message = None
        self.on_connect = None
        self.on_disconnect = None
        self.on_subscribe = None
        self.username = None
        self.password = None
        self.tls_context = None
        self.published = []      # (topic, payload, qos)
        self.subscribed = []     # (topic, qos)
        self.unsubscribed = []   # topic
        self._next_mid = 1
        self.loop_started = False
        # Tunable behavior for tests.
        self.auto_suback = True
        self.subscribe_rc = mqtt.MQTT_ERR_SUCCESS
        self.publish_rc = mqtt.MQTT_ERR_SUCCESS
        self.suback_failure_topics = set()
        FakeMqttClient.instances.append(self)

    # ---- configuration calls
    def username_pw_set(self, username, password):
        self.username = username
        self.password = password

    def tls_set_context(self, ctx):
        self.tls_context = ctx

    # ---- connection lifecycle
    def connect_async(self, host, port, keepalive):
        self.host = host
        self.port = port
        self.keepalive = keepalive
        self._connected = True

    def loop_start(self):
        self.loop_started = True

    def loop_stop(self):
        self.loop_started = False

    def disconnect(self):
        self._connected = False

    def is_connected(self):
        return self._connected

    # ---- pub/sub
    def subscribe(self, topic, qos=0):
        if self.subscribe_rc != mqtt.MQTT_ERR_SUCCESS:
            return (self.subscribe_rc, None)
        mid = self._next_mid
        self._next_mid += 1
        self.subscribed.append((topic, qos))
        granted = [0x80] if topic in self.suback_failure_topics else [qos]
        if self.auto_suback:
            # SUBACK arrives asynchronously, AFTER subscribe() returns and the
            # provider has registered mid->topic (mirrors a real broker).
            threading.Timer(
                0.02,
                lambda: self.on_subscribe and self.on_subscribe(self, None, mid, granted, None),
            ).start()
        return (mqtt.MQTT_ERR_SUCCESS, mid)

    def publish(self, topic, payload, qos=0):
        self.published.append((topic, payload, qos))
        return SimpleNamespace(rc=self.publish_rc)

    def unsubscribe(self, topic):
        self.unsubscribed.append(topic)

    # ---- test helpers
    def deliver(self, topic, body, qos=0):
        payload = body if isinstance(body, (bytes, bytearray)) else json.dumps(body).encode("utf-8")
        msg = SimpleNamespace(topic=topic, payload=payload, qos=qos)
        self.on_message(self, None, msg)


@pytest.fixture(autouse=True)
def _patch_paho(monkeypatch):
    FakeMqttClient.instances = []
    monkeypatch.setattr(sp.mqtt, "Client", FakeMqttClient)
    yield


def _local_only_config():
    cfg = MessagingConfiguration()
    cfg.messaging = MessagingConfigData(
        local=LocalMqttConfig(type="mqtt", host="localhost", port=1883, client_id="local-cid"),
        northbound=None,
    )
    return cfg


def _dual_config():
    cfg = MessagingConfiguration()
    cfg.messaging = MessagingConfigData(
        local=LocalMqttConfig(type="mqtt", host="localhost", port=1883, client_id="local-cid"),
        northbound=NorthboundMqttConfig(
            endpoint="northbound.example.com",
            port=8883,
            client_id="northbound-cid",
            credentials=CredentialsConfig(cert_path="c.pem", key_path="k.pem", ca_path="a.pem"),
        ),
    )
    return cfg


@pytest.fixture
def _patch_ssl(monkeypatch):
    """Avoid real cert file IO when the northbound/local TLS path runs."""
    import ssl as _ssl

    monkeypatch.setattr(_ssl, "create_default_context", lambda *a, **k: SimpleNamespace(
        load_verify_locations=lambda *a, **k: None,
        load_cert_chain=lambda *a, **k: None,
        check_hostname=True,
        verify_mode=None,
    ))
    yield


def _msg(name="M", payload=None):
    return MessageBuilder.create(name, "1.0").with_payload(payload or {}).with_tags({}).build()


# --------------------------------------------------------------------------- tests


class TestInitAndConnect:
    def test_local_only_initializes_and_connects(self):
        prov = StandaloneProvider(_local_only_config(), "thing-1")
        clients = prov.get_native_client()
        assert clients["local"] is not None
        assert clients["northbound"] is None
        assert clients["local"].host == "localhost"
        assert clients["local"].port == 1883
        assert clients["local"].loop_started is True
        # default client id from config
        assert clients["local"].client_id == "local-cid"
        prov.disconnect()

    def test_dual_broker_initializes_both(self, _patch_ssl):
        prov = StandaloneProvider(_dual_config(), "thing-1")
        clients = prov.get_native_client()
        assert clients["local"] is not None and clients["northbound"] is not None
        # Northbound has a CA path, so it must have a TLS context configured.
        assert clients["northbound"].tls_context is not None
        assert clients["northbound"].host == "northbound.example.com"
        prov.disconnect()

    def test_client_id_falls_back_to_thing_name(self):
        cfg = _local_only_config()
        cfg.messaging.local.client_id = None
        prov = StandaloneProvider(cfg, "my-thing")
        assert prov.get_native_client()["local"].client_id == "my-thing"
        prov.disconnect()

    def test_client_id_falls_back_to_default_when_no_thing(self):
        cfg = _local_only_config()
        cfg.messaging.local.client_id = None
        prov = StandaloneProvider(cfg, None)
        assert prov.get_native_client()["local"].client_id == "edgecommons"
        prov.disconnect()

    def test_local_username_password_set(self):
        cfg = _local_only_config()
        cfg.messaging.local.credentials = CredentialsConfig(username="u", password="p")
        prov = StandaloneProvider(cfg, "t")
        c = prov.get_native_client()["local"]
        assert c.username == "u" and c.password == "p"
        prov.disconnect()

    def test_local_tls_when_ca_present(self, _patch_ssl):
        cfg = _local_only_config()
        cfg.messaging.local.credentials = CredentialsConfig(ca_path="ca.pem")
        prov = StandaloneProvider(cfg, "t")
        assert prov.get_native_client()["local"].tls_context is not None
        prov.disconnect()

    def test_connect_timeout_raises(self, monkeypatch):
        # is_connected never returns True -> _connect_client times out.
        orig = FakeMqttClient.connect_async

        def never_connect(self, host, port, keepalive):
            self.host, self.port = host, port  # stays disconnected

        monkeypatch.setattr(FakeMqttClient, "connect_async", never_connect)
        # Speed up: patch time so the 5s wait loop trips immediately. Use an
        # ever-advancing clock (not a fixed-length iterator): patching sp.time.time
        # patches time.time process-wide, and the logging framework (log_cli=DEBUG)
        # also calls it, so a 3-value iter exhausts -> StopIteration on chattier runs.
        clock = {"t": 0.0}

        def fake_time():
            t = clock["t"]
            clock["t"] += 100.0
            return t

        monkeypatch.setattr(sp.time, "time", fake_time)
        monkeypatch.setattr(sp.time, "sleep", lambda s: None)
        with pytest.raises((TimeoutError, RuntimeError)):
            StandaloneProvider(_local_only_config(), "t")
        monkeypatch.setattr(FakeMqttClient, "connect_async", orig)


class TestTls:
    def test_northbound_without_ca_uses_plain_mqtt(self):
        cfg = _dual_config()
        cfg.messaging.northbound.credentials = CredentialsConfig(cert_path="c", key_path=None, ca_path=None)
        prov = StandaloneProvider(cfg, "t")
        assert prov.get_native_client()["northbound"].tls_context is None
        prov.disconnect()


class TestSubscribeDispatch:
    def test_subscribe_publish_and_dispatch(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        got = threading.Event()
        box = {}

        def cb(topic, msg):
            box["topic"] = topic
            box["msg"] = msg
            got.set()

        prov.subscribe("data/+", cb)
        # confirmed subscription is tracked
        assert "data/+" in prov._local.subscriptions
        prov.get_native_client()["local"].deliver("data/x", {"header": {"name": "N", "version": "1"}, "body": {"v": 1}})
        assert got.wait(2)
        assert box["topic"] == "data/x"
        assert box["msg"].get_body() == {"v": 1}
        prov.disconnect()

    def test_publish_serializes_message(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.publish("out/topic", _msg("Hello", {"a": 1}))
        pubs = prov.get_native_client()["local"].published
        assert len(pubs) == 1
        topic, payload, qos_value = pubs[0]
        assert topic == "out/topic"
        assert json.loads(payload)["body"] == {"a": 1}
        assert qos_value == 1
        prov.disconnect()

    def test_publish_raw(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.publish_raw("raw/topic", {"x": 9})
        topic, payload, qos_value = prov.get_native_client()["local"].published[0]
        assert json.loads(payload) == {"x": 9}
        assert qos_value == 1  # local raw uses QoS 1
        prov.disconnect()

    def test_non_json_payload_becomes_raw(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        got = threading.Event()
        box = {}
        prov.subscribe("t/raw", lambda topic, m: (box.__setitem__("m", m), got.set()))
        prov.get_native_client()["local"].deliver("t/raw", b"not-json-here")
        assert got.wait(2)
        assert box["m"].get_raw() == "not-json-here"
        prov.disconnect()

    def test_no_subscription_match_is_noop(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        # No subscription registered; should not raise.
        prov.get_native_client()["local"].deliver("nothing/here", {"body": 1})
        prov.disconnect()

    def test_unsubscribe_removes_subscription(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.subscribe("a/b", lambda t, m: None)
        assert "a/b" in prov._local.subscriptions
        prov.unsubscribe("a/b")
        assert "a/b" not in prov._local.subscriptions
        assert "a/b" in prov.get_native_client()["local"].unsubscribed
        prov.disconnect()

    def test_subscribe_rc_failure_raises(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.get_native_client()["local"].subscribe_rc = mqtt.MQTT_ERR_NO_CONN
        with pytest.raises(RuntimeError, match="subscription request"):
            prov.subscribe("x/y", lambda t, m: None)
        # pending entry cleaned up
        assert "x/y" not in prov._local.pending_subscriptions
        prov.disconnect()

    def test_subscribe_timeout_raises(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov._subscription_timeout = 0.1
        prov.get_native_client()["local"].auto_suback = False
        with pytest.raises(TimeoutError):
            prov.subscribe("slow/topic", lambda t, m: None)
        assert "slow/topic" not in prov._local.pending_subscriptions
        prov.disconnect()

    def test_suback_failure_still_unblocks(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.get_native_client()["local"].suback_failure_topics.add("bad/topic")
        # 0x80 granted Qos -> logged as failure but subscribe() still returns
        prov.subscribe("bad/topic", lambda t, m: None)
        assert "bad/topic" in prov._local.subscriptions
        prov.disconnect()

    def test_max_messages_drop_on_overflow(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        release = threading.Event()
        started = threading.Event()

        def slow_cb(topic, m):
            started.set()
            release.wait(2)

        # max_messages=1 -> only one in-flight; the second is dropped.
        prov.subscribe("q/+", slow_cb, max_messages=1)
        client = prov.get_native_client()["local"]
        client.deliver("q/1", {"body": 1})
        assert started.wait(2)  # first one is executing and holds the only permit
        # second arrives while permit held -> dropped (no exception, no second call)
        client.deliver("q/2", {"body": 2})
        release.set()
        prov.disconnect()


class TestRequestReply:
    def test_request_resolves_on_reply(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        iou = prov.request("svc/req", _msg("Q", {"q": 1}))
        reply_topic = iou.get_user_data()
        assert reply_topic.startswith("edgecommons/reply-")
        # the request was published
        client = prov.get_native_client()["local"]
        assert any(t == "svc/req" for (t, _p, _q) in client.published)
        # deliver a reply on the reply topic
        client.deliver(reply_topic, {"body": {"answer": 42}})
        done, reply = iou.get(2)
        assert done is True
        assert reply.get_body() == {"answer": 42}
        # one-shot reply subscription torn down
        assert reply_topic not in prov._local.subscriptions
        assert reply_topic in client.unsubscribed
        prov.disconnect()

    def test_reply_uses_reply_to_and_correlation(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        request = _msg("Req", {"q": 1})
        request.get_header().reply_to = "edgecommons/reply-xyz"
        request.set_correlation_id("corr-123")
        reply = _msg("Resp", {"a": 2})
        prov.reply(request, reply)
        client = prov.get_native_client()["local"]
        topic, payload, _qos = client.published[-1]
        assert topic == "edgecommons/reply-xyz"
        assert json.loads(payload)["header"]["correlation_id"] == "corr-123"
        prov.disconnect()

    def test_reply_without_reply_to_raises(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        request = _msg("Req", {"q": 1})
        request.get_header().reply_to = None
        with pytest.raises(ValueError, match="reply-to"):
            prov.reply(request, _msg("Resp", {}))
        prov.disconnect()

    def test_cancel_request_cleans_up(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        iou = prov.request("svc/req", _msg("Q", {"q": 1}))
        reply_topic = iou.get_user_data()
        assert reply_topic in prov._response_ious
        prov.cancel_request(iou)
        assert reply_topic not in prov._response_ious
        assert reply_topic not in prov._local.subscriptions
        prov.disconnect()


class TestIotCoreWrappers:
    def test_iotcore_publish_and_subscribe(self, _patch_ssl):
        prov = StandaloneProvider(_dual_config(), "t")
        prov.publish_northbound("iot/out", _msg("M", {"k": 1}), Qos.AT_LEAST_ONCE)
        iot = prov.get_native_client()["northbound"]
        topic, _payload, qos_value = iot.published[-1]
        assert topic == "iot/out" and qos_value == 1  # AT_LEAST_ONCE -> 1

        got = threading.Event()
        prov.subscribe_northbound("iot/in", lambda t, m: got.set(), Qos.AT_MOST_ONCE)
        # AT_MOST_ONCE subscribed at QoS 0
        assert ("iot/in", 0) in iot.subscribed
        iot.deliver("iot/in", {"body": 1})
        assert got.wait(2)
        prov.unsubscribe_northbound("iot/in")
        assert "iot/in" in iot.unsubscribed
        prov.disconnect()

    def test_iotcore_request_reply_and_cancel(self, _patch_ssl):
        prov = StandaloneProvider(_dual_config(), "t")
        iou = prov.request_northbound("iot/req", _msg("Q", {"q": 1}))
        reply_topic = iou.get_user_data()
        iot = prov.get_native_client()["northbound"]
        # request published at QoS 1
        assert any(t == "iot/req" and q == 1 for (t, _p, q) in iot.published)

        request = _msg("Req", {})
        request.get_header().reply_to = "edgecommons/reply-iot"
        prov.reply_northbound(request, _msg("Resp", {"a": 1}))
        assert any(t == "edgecommons/reply-iot" for (t, _p, _q) in iot.published)

        prov.cancel_request_northbound(iou)
        assert reply_topic not in prov._response_ious
        prov.disconnect()

    def test_iotcore_publish_raw(self, _patch_ssl):
        prov = StandaloneProvider(_dual_config(), "t")
        prov.publish_northbound_raw("iot/raw", {"z": 1}, Qos.AT_LEAST_ONCE)
        topic, payload, qos_value = prov.get_native_client()["northbound"].published[-1]
        assert topic == "iot/raw" and json.loads(payload) == {"z": 1} and qos_value == 1
        prov.disconnect()


class TestConnectedAndCallbacks:
    def test_connected_reflects_local_client(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        assert prov.connected() is True
        prov.get_native_client()["local"]._connected = False
        assert prov.connected() is False
        prov.disconnect()

    def test_connected_false_when_no_client(self):
        prov = StandaloneProvider.__new__(StandaloneProvider)
        prov._local = _BrokerChannel("local")
        assert prov.connected() is False

    def test_connected_false_on_exception(self):
        prov = StandaloneProvider.__new__(StandaloneProvider)
        prov._local = _BrokerChannel("local")
        boom = SimpleNamespace(is_connected=lambda: (_ for _ in ()).throw(RuntimeError("x")))
        prov._local.client = boom
        assert prov.connected() is False

    def test_on_connect_error_code_logged(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        # rc != 0 path: should not raise, just log
        prov._on_connect(prov._local, 5)
        prov.disconnect()

    def test_resubscribe_on_reconnect(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.subscribe("re/sub", lambda t, m: None)
        client = prov.get_native_client()["local"]
        before = list(client.subscribed)
        # simulate a reconnect (rc==0) -> existing subscriptions re-sent
        prov._on_connect(prov._local, 0)
        assert len(client.subscribed) > len(before)
        prov.disconnect()

    def test_on_disconnect_clears_pending(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        ev = threading.Event()
        prov._local.pending_subscriptions["p/topic"] = ev
        prov._local.mid_to_topic[7] = "p/topic"
        prov._on_disconnect(prov._local, 1)  # unexpected disconnect
        assert ev.is_set()
        assert prov._local.pending_subscriptions == {}
        assert prov._local.mid_to_topic == {}
        prov.disconnect()

    def test_on_disconnect_clean_code(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov._on_disconnect(prov._local, 0)  # clean disconnect log path
        prov.disconnect()


class TestPublishErrors:
    def test_publish_logs_on_rc_failure(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov.get_native_client()["local"].publish_rc = mqtt.MQTT_ERR_NO_CONN
        # should not raise even though rc indicates failure
        prov.publish("t/x", _msg("M", {"a": 1}))
        prov.disconnect()

    def test_require_client_raises_when_absent(self):
        prov = StandaloneProvider(_local_only_config(), "t")
        prov._northbound.client = None
        with pytest.raises(RuntimeError, match="Northbound"):
            prov.publish_northbound("x", _msg("M", {}), Qos.AT_MOST_ONCE)
        prov.disconnect()


class TestStaticHelpers:
    def test_mqtt_qos_mapping(self):
        assert StandaloneProvider._mqtt_qos(Qos.AT_MOST_ONCE) == 0
        assert StandaloneProvider._mqtt_qos(Qos.AT_LEAST_ONCE) == 1

    def test_make_semaphore(self):
        assert StandaloneProvider._make_semaphore(0) is None
        assert StandaloneProvider._make_semaphore(None) is None
        sem = StandaloneProvider._make_semaphore(2)
        assert isinstance(sem, type(threading.Semaphore()))

    def test_run_message_with_semaphore(self):
        calls = []
        sem = threading.Semaphore(1)
        permits = threading.Semaphore(1)
        StandaloneProvider._run_message(permits, sem, lambda t, m: calls.append((t, m)), "top", "m")
        assert calls == [("top", "m")]

    def test_run_message_releases_permit_on_error(self):
        permits = threading.Semaphore(1)

        def boom(t, m):
            raise ValueError("nope")

        with pytest.raises(ValueError):
            StandaloneProvider._run_message(permits, None, boom, "t", "m")
        # permit was released despite the error
        assert permits.acquire(blocking=False)
