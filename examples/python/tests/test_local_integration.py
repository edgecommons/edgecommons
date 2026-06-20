"""Local STANDALONE integration test for the Python component skeleton.

Exercises the ggcommons consumer API the skeleton relies on -- config access +
template substitution, messaging publish/subscribe round-trip, the GreengrassApp
wiring, metric definition, and heartbeat -- against a local MQTT broker
(EMQX on localhost:1883). No AWS / IoT Core required.

Run:
    python -m pytest tests/test_local_integration.py -v

Skipped automatically if no local broker is reachable on localhost:1883.
"""
import os
import socket
import threading

import pytest

TEST_DIR = os.path.dirname(os.path.abspath(__file__))
CONFIG_DIR = os.path.join(os.path.dirname(TEST_DIR), "test-configs")
COMPONENT_CONFIG = os.path.join(CONFIG_DIR, "config_local.json")
MESSAGING_CONFIG = os.path.join(CONFIG_DIR, "standalone-local.json")

THING = "skeleton-test-thing"
COMPONENT = "PythonComponentSkeleton"


def _broker_up(host="localhost", port=1883, timeout=1.0):
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


pytestmark = pytest.mark.skipif(
    not _broker_up(), reason="local MQTT broker not reachable on localhost:1883"
)


@pytest.fixture(scope="module")
def gg():
    """Build the framework once (MessagingClient/MetricEmitter are process-global)."""
    from ggcommons import GGCommonsBuilder

    args = [
        "-c", "FILE", COMPONENT_CONFIG,
        "-m", "STANDALONE", MESSAGING_CONFIG,
        "-t", THING,
    ]
    instance = GGCommonsBuilder.create(COMPONENT).with_args(args).build()
    yield instance
    instance.shutdown()


def test_config_manager_surface(gg):
    cm = gg.get_config_manager()
    assert cm.get_component_name() == COMPONENT
    assert cm.get_thing_name() == THING
    assert cm.get_global_config().get("publish_interval") == 1
    # Template substitution resolves component / thing / custom tags.
    resolved = cm.resolve_template("{ComponentName}/{ThingName}/{site}")
    assert resolved == f"{COMPONENT}/{THING}/site1"


def test_messaging_round_trip(gg):
    from ggcommons import MessagingClient
    from ggcommons.messaging.message_builder import MessageBuilder

    received = []
    done = threading.Event()

    def handler(topic, msg):
        received.append(msg)
        done.set()

    topic = "skeleton/test/roundtrip"
    MessagingClient.subscribe(topic, handler)
    msg = (
        MessageBuilder.create("RoundTrip", "1.0")
        .with_payload({"hello": "world"})
        .with_config(gg.get_config_manager())
        .build()
    )
    MessagingClient.publish(topic, msg)
    assert done.wait(5), "message should round-trip through the local broker"
    assert received[0].get_body()["hello"] == "world"
    MessagingClient.unsubscribe(topic)


def test_request_reply_round_trip(gg):
    from ggcommons import MessagingClient
    from ggcommons.messaging.message_builder import MessageBuilder

    cm = gg.get_config_manager()

    def responder(topic, request):
        reply = (
            MessageBuilder.create("Reply", "1.0")
            .with_payload({"answer": 42})
            .with_config(cm)
            .build()
        )
        MessagingClient.reply(request, reply)

    MessagingClient.subscribe("skeleton/test/req", responder)
    req = (
        MessageBuilder.create("Req", "1.0")
        .with_payload({"q": "x"})
        .with_config(cm)
        .build()
    )
    iou = MessagingClient.request("skeleton/test/req", req)
    done, reply = iou.get(5)
    assert done is True, "request should receive a reply over the local broker"
    assert reply.get_body()["answer"] == 42
    MessagingClient.unsubscribe("skeleton/test/req")


def test_cancel_request_carries_reply_topic(gg):
    """Tier B fix: the standalone request Iou carries its reply topic so
    cancel_request can tear down the right subscription (was Iou() -> None)."""
    from ggcommons import MessagingClient
    from ggcommons.messaging.message_builder import MessageBuilder

    cm = gg.get_config_manager()
    req = (
        MessageBuilder.create("Req", "1.0")
        .with_payload({"q": "x"})
        .with_config(cm)
        .build()
    )
    iou = MessagingClient.request("skeleton/test/never-answered", req)
    done, _pending = iou.get(1)  # no responder -> times out
    assert done is False
    user_data = iou.get_user_data()
    assert isinstance(user_data, str) and user_data.startswith("ggcommons/reply-")
    MessagingClient.cancel_request(iou)  # must not raise


def test_max_concurrency_cap_limits_callbacks(gg):
    """Tier D: standalone subscriptions honor the maxConcurrency cap (parity with
    the IPC handler / Java / Rust). With cap=2 and 6 queued messages, at most two
    callbacks run at once."""
    import time

    from ggcommons import MessagingClient
    from ggcommons.messaging.message_builder import MessageBuilder

    cm = gg.get_config_manager()
    topic = "skeleton/test/concurrency"
    messages = 6
    lock = threading.Lock()
    state = {"active": 0, "max": 0, "done": 0}
    finished = threading.Event()

    def handler(t, m):
        with lock:
            state["active"] += 1
            state["max"] = max(state["max"], state["active"])
        time.sleep(0.2)
        with lock:
            state["active"] -= 1
            state["done"] += 1
            if state["done"] >= messages:
                finished.set()

    MessagingClient.subscribe(topic, handler, 2)  # cap = 2
    for i in range(messages):
        MessagingClient.publish(
            topic,
            MessageBuilder.create("C", "1.0").with_payload({"i": i}).with_config(cm).build(),
        )
    assert finished.wait(15), "all messages should be processed"
    assert state["max"] <= 2, f"cap of 2 exceeded; observed {state['max']}"
    assert state["max"] >= 2, f"with 6 queued msgs and cap 2, concurrency should reach 2; observed {state['max']}"
    MessagingClient.unsubscribe(topic)


def test_raw_publish_delivers_non_envelope_payload(gg):
    """publish_raw sends a non-envelope payload; the subscriber receives it as a
    raw message (get_raw() set, get_body() None) -- parity with the Java/Rust raw
    handling, exercised over the local broker."""
    from ggcommons import MessagingClient

    topic = "skeleton/test/raw"
    received = []
    got = threading.Event()

    def handler(_t, m):
        received.append(m)
        got.set()

    MessagingClient.subscribe(topic, handler)
    MessagingClient.publish_raw(topic, {"sensor": "temp", "value": 21.5})
    assert got.wait(5), "raw message should be delivered"
    msg = received[0]
    assert msg.get_raw() == {"sensor": "temp", "value": 21.5}
    assert msg.get_body() is None
    MessagingClient.unsubscribe(topic)


def test_greengrass_app_constructs_and_defines_metric(gg):
    from app.greengrass_app import GreengrassApp

    app = GreengrassApp(config_manager=gg.get_config_manager())
    metric = app.define_metric()
    assert metric is not None


def test_metric_emits_to_log(gg):
    """A defined metric emitted through the configured 'log' target writes the
    metric log file -- exercises the metric pipeline locally (no AWS)."""
    import time
    from ggcommons.metrics.metric_emitter import MetricEmitter
    from ggcommons.metrics.metric_builder import MetricBuilder

    metric = (
        MetricBuilder.create("perf_local")
        .with_config(gg.get_config_manager())
        .add_measure("latency", "Milliseconds", 1)
        .build()
    )
    MetricEmitter.define_metric(metric)
    MetricEmitter.emit_metric("perf_local", {"latency": 12.5})

    log_path = os.path.join(os.path.dirname(TEST_DIR), "skeleton_test.metric.log")
    deadline = time.time() + 5
    while time.time() < deadline:
        if os.path.exists(log_path) and os.path.getsize(log_path) > 0:
            break
        time.sleep(0.25)
    assert os.path.exists(log_path) and os.path.getsize(log_path) > 0, (
        "emitting a defined metric should write the configured log target file"
    )
