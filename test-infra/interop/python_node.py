"""Cross-language interop node (Python) for edgecommons.

Two roles, talking to the shared local MQTT broker (localhost:1883) over a
local-only MQTT transport:

  python_node.py responder <request_topic>
      Subscribe to <request_topic>; reply to each request with
      {"echo": <request body>, "responder": "python"} (the lib copies the
      correlation id and publishes to the request's reply_to). Prints "READY"
      once subscribed, then runs until killed.

  python_node.py request <request_topic> <token>
      Send a request {"token": <token>, "from": "python"} and wait for the reply.
      Prints one JSON line and exits 0 on a correlated, well-formed reply, else 1.

UNS roles (M14 — UNS-CANONICAL-DESIGN §7):

  python_node.py uns-pub <identityJson> <class> [channel]
      Parse the wire-form identity with the lib's lenient parser, mint the topic
      with the real Uns builder (includeRoot=false), build a message stamped with
      that identity via the real MessageBuilder, publish it, and print one JSON
      line {"ok": true, "topic": <topic>, "envelope": <wire JSON>}.

  python_node.py uns-sub <topic>
      Subscribe to <topic> (prints READY), wait for one envelope, and print
      {"ok": <identity parsed>, "identity": <identity dict|null>, "body": <body>}.

  python_node.py uns-guard
      Attempt a raw publish to the reserved-class topic ecv1/dev1/comp1/main/state
      through the guarded public MessagingClient surface; exits NON-ZERO printing
      the reserved-topic error name. (The guard fires before the provider is
      touched, so this role needs no broker connection.)

Per-instance connectivity — one provider, two surfaces (pull + push):

  python_node.py status-responder <component>
      Run a real component named <component> that registers the canonical
      InstanceConnectivity provider; its built-in `status` verb answers out of that
      provider. Prints READY, then runs until killed.

  python_node.py status-request <component>
      Pull <component>'s built-in `status` verb over its command inbox
      (ecv1/interop-device/<component>/main/cmd/status) and print one JSON line
      {"ok": true, "reply_body": {"status": "RUNNING", "uptimeSecs": n, "instances": [...]}}.

  python_node.py state-instances-pub <component>
      The same component with the heartbeat enabled: the `state` keepalive PUSHES the
      very sample the `status` verb returns, in its `instances` array. Runs until killed.

  python_node.py state-instances-sub <component>
      Subscribe <component>'s reserved `state` topic (subscribing to a reserved class is
      allowed; only publishing to one is rejected), wait for the first RUNNING keepalive
      carrying a non-empty `instances[]`, and print
      {"ok": true, "state_status": "RUNNING", "instances": [...]}.
"""
import json
import os
import base64
import sys
import tempfile
import threading
import time
import uuid

from edgecommons.messaging.messaging_config import MessagingConfiguration
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider
from edgecommons.messaging.qos import Qos
from edgecommons.messaging.message import _binary_marker
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.identity import MessageIdentity
from edgecommons.command_inbox import CommandOutcome
from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
from edgecommons.uns import Uns, UnsClass
from edgecommons import EdgeCommons, LogRecord

LANG = "python"
HOST = os.environ.get("EDGECOMMONS_IT_MQTT_HOST", "localhost")
PORT = int(os.environ.get("EDGECOMMONS_IT_MQTT_PORT", "1883"))

# Canonical cross-language payload permutations: every language sends this as its request body's
# `types` field; the responder echoes it; test_interop asserts a deep, number-lenient round-trip in
# both directions. null is tested both inside an array and as a top-level map entry (`nullv`); since
# #15 the Java sender preserves null-valued Map entries, so explicit nulls round-trip four ways.
TYPES = {
    "b": True, "bf": False,
    "i": 42, "ni": -7, "fl": 3.5,
    "slash": "a/b", "quote": "x\"y",
    "arr": [1, "two", False, None],
    "nullv": None,
    "nested": {"k": [1, {"d": 2}]},
    "ea": [], "eo": {},
}


def _provider(suffix):
    cfg = {
        "messaging": {
            "local": {
                "type": "mqtt", "host": HOST, "port": PORT,
                "clientId": f"interop-{LANG}-{suffix}-{os.getpid()}",
            }
        }
    }
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(cfg, f)
        path = f.name
    try:
        config = MessagingConfiguration.load_from_file(path)
    finally:
        os.unlink(path)
    return StandaloneProvider(config, f"interop-{LANG}")


def _log_component_token():
    return f"interop-log-{LANG}"


def _write_command_runtime_config(component_token):
    """Create the minimal real runtime config used by the deferred-command responder."""
    cfg = {
        "component": {"token": component_token},
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": HOST,
                "port": PORT,
                "clientId": f"interop-{LANG}-deferred-runtime-{os.getpid()}",
            },
            "requestTimeoutSeconds": 4,
        },
        "heartbeat": {"enabled": False},
        "health": {"enabled": False},
    }
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(cfg, f)
        return f.name


def _write_log_runtime_config():
    component_token = _log_component_token()
    cfg = {
        "component": {"token": component_token},
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": HOST,
                "port": PORT,
                "clientId": f"interop-{LANG}-log-runtime-{os.getpid()}",
            },
            "requestTimeoutSeconds": 2,
        },
        "heartbeat": {"enabled": False},
        "health": {"enabled": False},
        "logging": {
            "level": "WARN",
            "publish": {
                "enabled": True,
                "destination": "local",
                "minLevel": "TRACE",
                "captureNative": False,
                "captureConsole": False,
                "redaction": {"enabled": False},
            },
        },
    }
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(cfg, f)
        return f.name


def _write_durable_acceptance_marker():
    """Persist the tiny P1 acceptance record before a deferred reply is activated.

    The interop role intentionally uses a fixed-size, uniquely named local marker rather
    than an in-memory flag, so the response's ``durablyAccepted`` claim follows a real
    filesystem durability boundary.  It is deleted only after the terminal reply has
    been attempted.
    """
    fd, marker_path = tempfile.mkstemp(
        prefix=f"edgecommons-p1-accept-{LANG}-", suffix=".marker"
    )
    try:
        with os.fdopen(fd, "wb") as marker:
            marker.write(b"accepted\n")
            marker.flush()
            os.fsync(marker.fileno())
    except BaseException:
        try:
            os.unlink(marker_path)
        except OSError:
            pass
        raise
    return marker_path


def _remove_durable_acceptance_marker(marker_path):
    """Best-effort cleanup after the accepted command has reached a terminal response."""
    try:
        os.unlink(marker_path)
    except OSError:
        pass


def _log_runtime_args(path):
    return [
        "--platform",
        "HOST",
        "--transport",
        "MQTT",
        path,
        "-c",
        "FILE",
        path,
        "-t",
        "interop-device",
    ]


def _wire_identity_device(identity):
    if not isinstance(identity, dict):
        return None
    hier = identity.get("hier")
    if not isinstance(hier, list) or not hier:
        return None
    tail = hier[-1]
    return tail.get("value") if isinstance(tail, dict) else None


def run_responder(topic):
    prov = _provider("resp")

    def handler(_t, request):
        reply = (
            MessageBuilder.create("InteropReply", "1.0")
            .with_payload({"echo": request.get_body(), "responder": LANG})
            .with_tags({})
            .build()
        )
        prov.reply(request, reply)

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        prov.disconnect()


def run_request(topic, token):
    prov = _provider("req")
    try:
        req = (
            MessageBuilder.create("InteropRequest", "1.0")
            .with_payload({"token": token, "from": LANG, "types": TYPES})
            .with_tags({})
            .build()
        )
        corr = req.get_correlation_id()
        done, reply = prov.request(topic, req).get(8)
        if not done or reply is None:
            print(json.dumps({"ok": False, "error": "timeout"}))
            return 1
        body = reply.get_body()
        match = reply.get_correlation_id() == corr
        print(json.dumps({"ok": True, "correlation_match": match, "reply_body": body}))
        ok = (
            match
            and isinstance(body, dict)
            and body.get("responder")
            and isinstance(body.get("echo"), dict)
            and body["echo"].get("token") == token
        )
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_raw_sub(topic, token):
    prov = _provider("rawsub")
    state = {}
    got = __import__("threading").Event()

    def handler(_t, m):
        state["body"] = m.get_body()
        state["raw"] = m.get_raw()
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(10):
            print(json.dumps({"ok": True, "delivered": False, "error": "timeout"}), flush=True)
            return 0
        print(json.dumps({
            "ok": False,
            "delivered": True,
            "raw": state.get("raw"),
            "body": state.get("body"),
            "expected_token": token,
        }), flush=True)
        return 1
    finally:
        prov.disconnect()


def run_raw_pub(topic, token):
    prov = _provider("rawpub")
    try:
        prov.publish_raw(topic, {"token": token, "from": LANG})
        time.sleep(0.5)  # let the QoS-0 publish drain before disconnect
        return 0
    finally:
        prov.disconnect()


def run_binary_sub(topic, expected_hex):
    prov = _provider("binsub")
    state = {}
    got = threading.Event()

    def handler(_t, m):
        try:
            data = m.get_binary_body() if m.is_binary_body() else None
            state["result"] = {
                "is_binary": m.is_binary_body(),
                "hex": data.hex() if data is not None else None,
            }
        except Exception as exc:  # pragma: no cover - exercised by subprocess harness
            state["result"] = {"is_binary": False, "hex": None, "error": str(exc)}
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(10):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        result = state["result"]
        ok = result["is_binary"] and result["hex"] == expected_hex.lower()
        result["ok"] = bool(ok)
        print(json.dumps(result), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_binary_pub(topic, body_hex):
    prov = _provider("binpub")
    try:
        msg = (
            MessageBuilder.create("InteropBinary", "1.0")
            .with_payload(bytes.fromhex(body_hex))
            .with_tags({"from": LANG})
            .build()
        )
        prov.publish(topic, msg)
        time.sleep(0.5)  # let the QoS-0 publish drain before disconnect
        return 0
    finally:
        prov.disconnect()


def _typed_body(body_hex):
    return {
        "signal": {"id": "camera-1/roi-17/thumbnail", "name": "Thumbnail"},
        "samples": [{
            "value": _binary_marker(bytes.fromhex(body_hex)),
            "quality": "GOOD",
            "sourceTsMs": 1783360799900,
            "serverTsMs": 1783360800000,
        }],
    }


def run_typed_sub(topic, expected_hex):
    prov = _provider("typedsub")
    state = {}
    got = threading.Event()

    def handler(_t, m):
        try:
            body = m.get_body()
            marker = body["samples"][0]["value"]["_edgecommonsBinary"]
            data = base64.b64decode(marker["data"])
            state["result"] = {
                "body_case": m.get_body_case().name,
                "hex": data.hex(),
                "source_ts_ms": body["samples"][0].get("sourceTsMs"),
                "server_ts_ms": body["samples"][0].get("serverTsMs"),
                "tag_from": m.get_tags().to_dict().get("from") if m.get_tags() else None,
            }
        except Exception as exc:  # pragma: no cover - exercised by subprocess harness
            state["result"] = {"body_case": None, "hex": None, "error": str(exc)}
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(10):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        result = state["result"]
        ok = (
            result["body_case"] == "SOUTHBOUND_SIGNAL_UPDATE"
            and result["hex"] == expected_hex.lower()
            and result["source_ts_ms"] == 1783360799900
            and result["server_ts_ms"] == 1783360800000
        )
        result["ok"] = bool(ok)
        print(json.dumps(result), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_typed_pub(topic, body_hex):
    prov = _provider("typedpub")
    try:
        msg = (
            MessageBuilder.create("SouthboundSignalUpdate", "1.0")
            .with_southbound_signal_update(_typed_body(body_hex))
            .with_tags({"from": LANG})
            .build()
        )
        prov.publish(topic, msg)
        time.sleep(0.5)
        return 0
    finally:
        prov.disconnect()


def run_deferred_responder(component_token):
    """Serve a real command inbox whose reply is deferred until acceptance is explicit.

    The marker models a completed durable insert in this wire-only harness.  Keeping that
    state transition immediately before ``activate`` makes the activation ordering observable
    in every language without pretending that a timer is durable work.
    """
    path = _write_command_runtime_config(component_token)
    gg = None
    try:
        gg = EdgeCommons(
            f"com.mbreissi.edgecommons.interop.{LANG}.DeferredResponder",
            _log_runtime_args(path),
        )
        inbox = gg.get_commands()
        if inbox is None:
            raise RuntimeError("runtime did not expose command inbox")

        def deferred_handler(request):
            token = inbox.defer(request, 4)
            try:
                acceptance_marker = _write_durable_acceptance_marker()
            except OSError:
                token.discard()
                return CommandOutcome.error("ACCEPTANCE_FAILED", "work was not accepted")
            if not token.activate():
                _remove_durable_acceptance_marker(acceptance_marker)
                return CommandOutcome.error("ACTIVATION_FAILED", "deferred token was not open")

            def settle_after_acceptance():
                try:
                    token.settle_success({
                        "token": request.get_body().get("token"),
                        "responder": LANG,
                        "durablyAccepted": True,
                    })
                finally:
                    _remove_durable_acceptance_marker(acceptance_marker)

            return CommandOutcome.deferred_with_continuation(token, settle_after_acceptance)

        inbox.register_outcome("deferred", deferred_handler)
        print("READY", flush=True)
        while True:
            time.sleep(1)
    finally:
        if gg is not None:
            gg.shutdown()
        try:
            os.unlink(path)
        except OSError:
            pass


def run_deferred_request(topic, token):
    """Send one command and retain the reply subscription long enough to reject duplicates."""
    prov = _provider("deferredreq")
    received = []
    received_lock = threading.Lock()
    first_reply = threading.Event()
    reply_topic = f"interop/deferred/reply/{LANG}/{uuid.uuid4().hex}"
    try:
        def on_reply(_topic, reply):
            with received_lock:
                received.append({
                    "correlation": reply.get_correlation_id(),
                    "body": reply.get_body(),
                })
            first_reply.set()

        prov.subscribe(reply_topic, on_reply)
        request = (
            MessageBuilder.create("deferred", "1.0")
            .with_command({"token": token, "from": LANG})
            .with_reply_to(reply_topic)
            .with_tags({})
            .build()
        )
        correlation = request.get_correlation_id()
        prov.publish(topic, request)
        if not first_reply.wait(8):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        # The command inbox must settle exactly once.  Retain the subscription rather than
        # treating the first arrival as proof that a buggy double-settlement cannot follow.
        time.sleep(0.75)
        with received_lock:
            replies = list(received)
        first = replies[0]
        body = first["body"]
        result = body.get("result") if isinstance(body, dict) else None
        ok = (
            len(replies) == 1
            and first["correlation"] == correlation
            and isinstance(body, dict)
            and body.get("ok") is True
            and isinstance(result, dict)
            and result.get("token") == token
            and result.get("durablyAccepted") is True
            and bool(result.get("responder"))
        )
        print(json.dumps({
            "ok": ok,
            "reply_count": len(replies),
            "correlation_match": first["correlation"] == correlation,
            "reply_body": body,
        }), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_confirmed_sub(topic, token):
    """Receive one strict-publication envelope and reject a second payload."""
    prov = _provider("confirmedsub")
    received = []
    received_lock = threading.Lock()
    first_message = threading.Event()
    try:
        def on_message(_topic, message):
            with received_lock:
                received.append(message.get_body())
            first_message.set()

        prov.subscribe(topic, on_message)
        print("READY", flush=True)
        if not first_message.wait(8):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        time.sleep(0.75)
        with received_lock:
            messages = list(received)
        body = messages[0]
        ok = (
            len(messages) == 1
            and isinstance(body, dict)
            and body.get("token") == token
            and bool(body.get("from"))
        )
        print(json.dumps({"ok": ok, "message_count": len(messages), "body": body}), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_confirmed_pub(topic, token):
    """Return success only after the provider's QoS1 PUBACK wait completes."""
    prov = _provider("confirmedpub")
    try:
        message = (
            MessageBuilder.create("InteropConfirmed", "1.0")
            .with_payload({"token": token, "from": LANG})
            .with_tags({})
            .build()
        )
        prov.publish_confirmed(topic, message.to_bytes(), Qos.AT_LEAST_ONCE, 5)
        print(json.dumps({"ok": True, "confirmed": True, "qos": 1}), flush=True)
        return 0
    except Exception as exc:  # pragma: no cover - exercised by the broker-backed harness
        print(json.dumps({"ok": False, "error": type(exc).__name__}), flush=True)
        return 1
    finally:
        prov.disconnect()


def run_log_sub(topic, token):
    prov = _provider("logsub")
    state = {}
    got = threading.Event()

    def handler(t, m):
        try:
            body = m.get_body()
            identity = m.get_identity().to_dict() if m.get_identity() else None
            header = m.get_header().to_dict() if m.get_header() else None
            fields = body.get("fields", {}) if isinstance(body, dict) else {}
            ok = (
                t == topic
                and isinstance(body, dict)
                and body.get("schema") == "edgecommons.log.v1"
                and body.get("level") == "WARN"
                and body.get("message") == f"log-interop-{token}"
                and fields.get("nonce") == token
                and identity is not None
                and _wire_identity_device(identity) == "interop-device"
                and identity.get("component", "").startswith("interop-log-")
                # Component scope (D-U28): the wire identity omits `instance` entirely.
                and "instance" not in identity
                and header is not None
                and header.get("name") == "log"
                and header.get("version") == "1.0"
            )
            state["result"] = {
                "ok": bool(ok),
                "topic": t,
                "header": header,
                "identity": identity,
                "body": body,
            }
        except Exception as exc:  # pragma: no cover - exercised by subprocess harness
            state["result"] = {"ok": False, "error": str(exc)}
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(10):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        print(json.dumps(state["result"]), flush=True)
        return 0 if state["result"].get("ok") else 1
    finally:
        prov.disconnect()


def run_log_pub(token):
    path = _write_log_runtime_config()
    gg = None
    try:
        gg = EdgeCommons(
            f"com.mbreissi.edgecommons.interop.{LANG}.LogPublisher",
            _log_runtime_args(path),
        )
        gg.logs().publish(
            LogRecord(
                level="WARN",
                logger=f"interop.{LANG}",
                message=f"log-interop-{token}",
                fields={"nonce": token, "publisher": LANG},
            )
        )
        ok = gg.logs().flush(timeout=5)
        stats = gg.logs().stats()
        print(
            json.dumps(
                {
                    "ok": bool(ok and stats.get("published", 0) >= 1),
                    "component": _log_component_token(),
                    "stats": stats,
                }
            ),
            flush=True,
        )
        return 0 if ok and stats.get("published", 0) >= 1 else 1
    finally:
        if gg is not None:
            gg.shutdown()
        try:
            os.unlink(path)
        except OSError:
            pass


def _gg_topic(run_id, publisher, subscriber):
    return f"edgecommons/interop/binary/{run_id}/{publisher}/{subscriber}"


def _gg_typed_topic(run_id, publisher, subscriber):
    return f"edgecommons/interop/typed/{run_id}/{publisher}/{subscriber}"


def _publisher_from_gg_topic(topic):
    parts = topic.split("/")
    return parts[-2] if len(parts) >= 2 else None


def _gg_ready_path(run_id, lang):
    return f"/tmp/edgecommons_gg_ipc_binary_ready_{lang}_{run_id}"


def _gg_wait_for_ready(run_id, expected_langs):
    ready_wait = float(os.environ.get("EDGECOMMONS_GG_READY_WAIT_SECS", "180"))
    deadline = time.monotonic() + ready_wait
    while time.monotonic() < deadline:
        missing = [
            lang for lang in expected_langs
            if not os.path.exists(_gg_ready_path(run_id, lang))
        ]
        if not missing:
            return []
        time.sleep(0.2)
    return [
        lang for lang in expected_langs
        if not os.path.exists(_gg_ready_path(run_id, lang))
    ]


def _gg_log_ready_path(run_id, lang):
    return f"/tmp/edgecommons_gg_ipc_log_ready_{lang}_{run_id}"


def _gg_log_wait_for_ready(run_id, expected_langs):
    ready_wait = float(os.environ.get("EDGECOMMONS_GG_READY_WAIT_SECS", "180"))
    deadline = time.monotonic() + ready_wait
    while time.monotonic() < deadline:
        missing = [
            lang for lang in expected_langs
            if not os.path.exists(_gg_log_ready_path(run_id, lang))
        ]
        if not missing:
            return []
        time.sleep(0.2)
    return [
        lang for lang in expected_langs
        if not os.path.exists(_gg_log_ready_path(run_id, lang))
    ]


def _gg_log_runtime_args(path):
    return [
        "--platform",
        "GREENGRASS",
        "--transport",
        "IPC",
        "-c",
        "FILE",
        path,
        "-t",
        "interop-device",
    ]


def _gg_p1_ready_path(run_id, actor):
    return f"/tmp/edgecommons_gg_ipc_p1_ready_{actor}_{run_id}"


def _gg_p1_wait_for_ready(run_id, expected_actors):
    ready_wait = float(os.environ.get("EDGECOMMONS_GG_READY_WAIT_SECS", "180"))
    deadline = time.monotonic() + ready_wait
    while time.monotonic() < deadline:
        missing = [
            actor for actor in expected_actors
            if not os.path.exists(_gg_p1_ready_path(run_id, actor))
        ]
        if not missing:
            return []
        time.sleep(0.2)
    return [
        actor for actor in expected_actors
        if not os.path.exists(_gg_p1_ready_path(run_id, actor))
    ]


def _gg_p1_runtime_config(component_token):
    """Create a real GREENGRASS/IPC command-runtime configuration."""
    cfg = {
        "component": {"token": component_token},
        "messaging": {"requestTimeoutSeconds": 4},
        "heartbeat": {"enabled": False},
        "health": {"enabled": False},
    }
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(cfg, f)
        return f.name


def _gg_p1_runtime_args(path):
    return [
        "--platform",
        "GREENGRASS",
        "--transport",
        "IPC",
        "-c",
        "FILE",
        path,
        "-t",
        "interop-device",
    ]


def _gg_p1_target_actor(target_language, sender_actor):
    """Route the Rust self-pair through its separate deployed Rust principal."""
    if target_language == "rust" and sender_actor == "rust":
        return "rustpeer"
    return target_language



def _gg_p1_command_topic(actor):
    return f"ecv1/interop-device/interop-p1-{actor}/main/cmd/deferred"



def _gg_p1_confirmed_topic(run_id, publisher, target_actor):
    return f"edgecommons/interop/p1/{run_id}/confirmed/{publisher}/{target_actor}"


def run_gg_p1_matrix(run_id, langs_csv, _unused=None):
    """Run real IPC deferred-command and strict-confirmed-publish P1 behavior.

    Four canonical components act as requesters and strict publishers.  A second Rust
    component is deliberately used only for the Rust-to-Rust leg, giving that logical
    pair two distinct Greengrass principals instead of relying on self-delivery.
    """
    from edgecommons.messaging.providers.greengrass.greengrass_ipc import GreengrassIpcProvider

    languages = [part for part in langs_csv.split(",") if part]
    expected_actors = [
        part for part in os.environ.get(
            "EDGECOMMONS_GG_READY_LANGS", langs_csv
        ).split(",") if part
    ]
    actor = os.environ.get("EDGECOMMONS_GG_READY_LANG", LANG)
    canonical_actor = actor != "rustpeer"
    subscribe_delay = float(os.environ.get("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS", "2"))
    wait_secs = float(os.environ.get("EDGECOMMONS_GG_WAIT_SECS", "90"))
    duplicate_window = 0.75
    provider = GreengrassIpcProvider(receive_own_messages=True)
    received_confirmed = {}
    errors = {}
    received_lock = threading.Lock()
    first_confirmed = threading.Event()
    runtime = None
    config_path = None

    # Rust publishes its logical self-pair to the separate ``rustpeer`` component.  The
    # canonical Rust component therefore receives the other three logical publishers;
    # rustpeer's evidence carries the Rust-to-Rust receipt.
    expected_publishers = (
        [publisher for publisher in languages if publisher != "rust"]
        if actor == "rust"
        else (languages if canonical_actor else ["rust"])
    )

    def confirmed_handler(topic, message):
        publisher = _publisher_from_gg_topic(topic) or "unknown"
        try:
            body = message.get_body()
            valid = (
                isinstance(body, dict)
                and body.get("runId") == run_id
                and body.get("publisher") == publisher
                and body.get("targetActor") == actor
                and body.get("strict") is True
            )
            with received_lock:
                received_confirmed.setdefault(publisher, []).append({
                    "ok": bool(valid), "body": body, "topic": topic,
                })
            first_confirmed.set()
        except Exception as exc:  # pragma: no cover - exercised on device
            with received_lock:
                errors[f"confirmed:{publisher}"] = str(exc)
            first_confirmed.set()

    def send_deferred(target_language, target_actor):
        token = f"{run_id}:{actor}->{target_language}"
        reply_topic = (
            f"edgecommons/interop/p1/{run_id}/reply/{actor}/"
            f"{target_actor}/{uuid.uuid4().hex}"
        )
        replies = []
        reply_lock = threading.Lock()
        first_reply = threading.Event()

        def on_reply(_topic, reply):
            with reply_lock:
                replies.append({
                    "correlation": reply.get_correlation_id(),
                    "body": reply.get_body(),
                })
            first_reply.set()

        provider.subscribe(reply_topic, on_reply, max_concurrency=1, max_messages=2)
        request = (
            MessageBuilder.create("deferred", "1.0")
            .with_command({"token": token, "from": LANG, "actor": actor})
            .with_reply_to(reply_topic)
            .with_tags({})
            .build()
        )
        correlation = request.get_correlation_id()
        provider.publish(_gg_p1_command_topic(target_actor), request)
        if not first_reply.wait(8):
            return {"ok": False, "target_actor": target_actor, "error": "timeout"}
        time.sleep(duplicate_window)
        with reply_lock:
            captured = list(replies)
        first = captured[0]
        body = first["body"]
        result = body.get("result") if isinstance(body, dict) else None
        correlation_match = first["correlation"] == correlation
        ok = (
            len(captured) == 1
            and correlation_match
            and isinstance(body, dict)
            and body.get("ok") is True
            and isinstance(result, dict)
            and result.get("token") == token
            and result.get("durablyAccepted") is True
            and result.get("responder") == target_language
            and result.get("responderActor") == target_actor
        )
        return {
            "ok": bool(ok),
            "target_actor": target_actor,
            "expected_token": token,
            "expected_responder": target_language,
            "expected_responder_actor": target_actor,
            "reply_count": len(captured),
            "correlation_match": correlation_match,
            "duplicate_window_ms": int(duplicate_window * 1000),
            "reply_body": body,
        }

    try:
        config_path = _gg_p1_runtime_config(f"interop-p1-{actor}")
        runtime = EdgeCommons(
            f"com.mbreissi.edgecommons.interop.{LANG}.P1Responder",
            _gg_p1_runtime_args(config_path),
        )
        inbox = runtime.get_commands()
        if inbox is None:
            raise RuntimeError("runtime did not expose command inbox")

        def deferred_handler(request):
            token = inbox.defer(request, 4)
            request_body = request.get_body()
            try:
                acceptance_marker = _write_durable_acceptance_marker()
            except OSError:
                token.discard()
                return CommandOutcome.error("ACCEPTANCE_FAILED", "work was not accepted")
            if not token.activate():
                _remove_durable_acceptance_marker(acceptance_marker)
                return CommandOutcome.error("ACTIVATION_FAILED", "deferred token was not open")

            def settle_after_acceptance():
                try:
                    token.settle_success({
                        "token": request_body.get("token") if isinstance(request_body, dict) else None,
                        "responder": LANG,
                        "responderActor": actor,
                        "durablyAccepted": True,
                    })
                finally:
                    _remove_durable_acceptance_marker(acceptance_marker)

            return CommandOutcome.deferred_with_continuation(token, settle_after_acceptance)

        inbox.register_outcome("deferred", deferred_handler)
        provider.subscribe(
            f"edgecommons/interop/p1/{run_id}/confirmed/+/{actor}",
            confirmed_handler,
            max_concurrency=1,
            max_messages=32,
        )
        print("READY", flush=True)
        with open(_gg_p1_ready_path(run_id, actor), "w", encoding="utf-8") as f:
            f.write(str(time.time()))

        ready_missing = _gg_p1_wait_for_ready(run_id, expected_actors)
        deferred_requests = {}
        confirmed_publishes = {}
        if not ready_missing and canonical_actor:
            time.sleep(subscribe_delay)
            for target_language in languages:
                target_actor = _gg_p1_target_actor(target_language, actor)
                try:
                    deferred_requests[target_language] = send_deferred(
                        target_language, target_actor
                    )
                except Exception as exc:  # pragma: no cover - exercised on device
                    deferred_requests[target_language] = {
                        "ok": False, "target_actor": target_actor, "error": type(exc).__name__,
                    }
                message = (
                    MessageBuilder.create("InteropConfirmed", "1.0")
                    .with_payload({
                        "runId": run_id,
                        "publisher": LANG,
                        "publisherActor": actor,
                        "targetLanguage": target_language,
                        "targetActor": target_actor,
                        "strict": True,
                    })
                    .with_tags({})
                    .build()
                )
                try:
                    provider.publish_confirmed(
                        _gg_p1_confirmed_topic(run_id, LANG, target_actor),
                        message.to_bytes(), Qos.AT_LEAST_ONCE, 5,
                    )
                    confirmed_publishes[target_language] = {
                        "ok": True, "target_actor": target_actor, "confirmed": True, "qos": 1,
                    }
                except Exception as exc:  # pragma: no cover - exercised on device
                    confirmed_publishes[target_language] = {
                        "ok": False, "target_actor": target_actor, "error": type(exc).__name__,
                    }

        first_confirmed.wait(wait_secs)
        deadline = time.monotonic() + wait_secs
        while time.monotonic() < deadline:
            with received_lock:
                complete = all(publisher in received_confirmed for publisher in expected_publishers)
            if complete:
                break
            time.sleep(0.05)
        time.sleep(duplicate_window)
        with received_lock:
            received = {
                publisher: {"count": len(items), "items": items,
                            "ok": len(items) == 1 and items[0].get("ok") is True}
                for publisher, items in received_confirmed.items()
            }
            confirmed_missing = [
                publisher for publisher in expected_publishers if publisher not in received
            ]
            receive_ok = not confirmed_missing and all(
                received[publisher]["ok"] for publisher in expected_publishers
            )
            requests_ok = (not canonical_actor) or (
                len(deferred_requests) == len(languages)
                and all(item.get("ok") for item in deferred_requests.values())
            )
            publishes_ok = (not canonical_actor) or (
                len(confirmed_publishes) == len(languages)
                and all(item.get("ok") for item in confirmed_publishes.values())
            )
            ok = bool(
                not ready_missing and not errors and requests_ok and publishes_ok and receive_ok
            )
            result = {
                "schema": "edgecommons.gg-ipc-p1.v1",
                "ok": ok,
                "run_id": run_id,
                "actor": actor,
                "language": LANG,
                "canonical_actor": canonical_actor,
                "ready_missing": ready_missing,
                "deferred_requests": deferred_requests,
                "confirmed_publishes": confirmed_publishes,
                "confirmed_received": received,
                "confirmed_missing": confirmed_missing,
                "errors": errors,
            }
        result_path = f"/tmp/edgecommons_gg_ipc_p1_{actor}_{run_id}.json"
        with open(result_path, "w", encoding="utf-8") as f:
            json.dump(result, f, sort_keys=True)
        print(json.dumps(result, sort_keys=True), flush=True)
        return 0 if ok else 1
    finally:
        if runtime is not None:
            runtime.shutdown()
        if config_path:
            try:
                os.unlink(config_path)
            except OSError:
                pass
        provider.disconnect()


def run_gg_log_matrix(run_id, langs_csv, _unused=None):
    from edgecommons.messaging.providers.greengrass.greengrass_ipc import GreengrassIpcProvider

    expected_langs = [p for p in langs_csv.split(",") if p]
    ready_langs = [
        p for p in os.environ.get("EDGECOMMONS_GG_READY_LANGS", langs_csv).split(",") if p
    ]
    ready_lang = os.environ.get("EDGECOMMONS_GG_READY_LANG", LANG)
    subscribe_delay = float(os.environ.get("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS", "8"))
    wait_secs = float(os.environ.get("EDGECOMMONS_GG_WAIT_SECS", "35"))
    prov = GreengrassIpcProvider(receive_own_messages=True)
    received = {}
    errors = {}
    lock = threading.Lock()
    done = threading.Event()

    def maybe_done():
        if set(received) >= set(expected_langs):
            done.set()

    def log_handler(topic, m):
        try:
            body = m.get_body()
            identity = m.get_identity().to_dict() if m.get_identity() else None
            publisher = (identity or {}).get("component", "").removeprefix("interop-log-")
            fields = body.get("fields", {}) if isinstance(body, dict) else {}
            ok = (
                publisher in expected_langs
                and _wire_identity_device(identity) == "interop-device"
                and (identity or {}).get("instance") == "main"
                and body.get("schema") == "edgecommons.log.v1"
                and body.get("level") == "WARN"
                and body.get("logger") == f"interop.{publisher}"
                and body.get("message") == f"gg-log-interop-{run_id}-{publisher}"
                and fields.get("runId") == run_id
                and fields.get("publisher") == publisher
            )
            with lock:
                if publisher:
                    received[publisher] = {
                        "ok": bool(ok),
                        "topic": topic,
                        "identity": identity,
                        "body": body,
                    }
                maybe_done()
        except Exception as exc:  # pragma: no cover - exercised on device
            with lock:
                errors[f"log:{topic}"] = str(exc)
                maybe_done()

    prov.subscribe(
        "ecv1/interop-device/+/main/log/warn",
        log_handler,
        max_concurrency=1,
        max_messages=64,
    )
    print("READY", flush=True)
    with open(_gg_log_ready_path(run_id, ready_lang), "w", encoding="utf-8") as f:
        f.write(str(time.time()))
    gg = None
    path = None
    try:
        ready_missing = _gg_log_wait_for_ready(run_id, ready_langs)
        time.sleep(subscribe_delay)
        stats = {}
        if not ready_missing:
            path = _write_log_runtime_config()
            gg = EdgeCommons(
                f"com.mbreissi.edgecommons.interop.{LANG}.LogPublisher",
                _gg_log_runtime_args(path),
            )
            gg.logs().publish(
                LogRecord(
                    level="WARN",
                    logger=f"interop.{LANG}",
                    message=f"gg-log-interop-{run_id}-{LANG}",
                    fields={"runId": run_id, "publisher": LANG},
                )
            )
            gg.logs().flush(timeout=5)
            stats = gg.logs().stats()
        done.wait(wait_secs)
        with lock:
            missing = [lang for lang in expected_langs if lang not in received]
            ok = not ready_missing and not missing and not errors and all(
                received.get(lang, {}).get("ok") for lang in expected_langs
            )
            result = {
                "ok": bool(ok),
                "lang": LANG,
                "run_id": run_id,
                "ready_missing": ready_missing,
                "received": received,
                "missing": missing,
                "errors": errors,
                "published": stats,
            }
        result_path = f"/tmp/edgecommons_gg_ipc_log_{ready_lang}_{run_id}.json"
        with open(result_path, "w", encoding="utf-8") as f:
            json.dump(result, f, sort_keys=True)
        print(json.dumps(result, sort_keys=True), flush=True)
        return 0 if ok else 1
    finally:
        if gg is not None:
            gg.shutdown()
        if path:
            try:
                os.unlink(path)
            except OSError:
                pass
        prov.disconnect()


def run_gg_binary_matrix(run_id, langs_csv, expected_hex):
    from edgecommons.messaging.providers.greengrass.greengrass_ipc import GreengrassIpcProvider

    expected_langs = [p for p in langs_csv.split(",") if p]
    ready_langs = [
        p for p in os.environ.get("EDGECOMMONS_GG_READY_LANGS", langs_csv).split(",") if p
    ]
    ready_lang = os.environ.get("EDGECOMMONS_GG_READY_LANG", LANG)
    expected_bytes = bytes.fromhex(expected_hex)
    subscribe_delay = float(os.environ.get("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS", "8"))
    wait_secs = float(os.environ.get("EDGECOMMONS_GG_WAIT_SECS", "35"))
    prov = GreengrassIpcProvider(receive_own_messages=True)
    received = {}
    received_typed = {}
    errors = {}
    lock = threading.Lock()
    done = threading.Event()

    def maybe_done():
        if (
            set(received) >= set(expected_langs)
            and set(received_typed) >= set(expected_langs)
        ):
            done.set()

    def binary_handler(topic, m):
        publisher = _publisher_from_gg_topic(topic) or "unknown"
        try:
            is_binary = m.is_binary_body()
            data = m.get_binary_body() if is_binary else None
            hex_value = data.hex() if data is not None else None
            ok = is_binary and data == expected_bytes
            with lock:
                received[publisher] = {
                    "is_binary": is_binary,
                    "hex": hex_value,
                    "ok": ok,
                }
                maybe_done()
        except Exception as exc:  # pragma: no cover - exercised on device
            with lock:
                errors[f"{publisher}:binary"] = str(exc)
                received[publisher] = {"is_binary": False, "hex": None, "ok": False}
                maybe_done()

    def typed_handler(topic, m):
        publisher = _publisher_from_gg_topic(topic) or "unknown"
        try:
            body = m.get_body()
            sample = body["samples"][0]
            marker = sample["value"]["_edgecommonsBinary"]
            data = base64.b64decode(marker["data"])
            tag_from = m.get_tags().to_dict().get("from") if m.get_tags() else None
            body_case = m.get_body_case().name
            item = {
                "body_case": body_case,
                "hex": data.hex(),
                "source_ts_ms": sample.get("sourceTsMs"),
                "server_ts_ms": sample.get("serverTsMs"),
                "tag_from": tag_from,
            }
            item["ok"] = (
                body_case == "SOUTHBOUND_SIGNAL_UPDATE"
                and data == expected_bytes
                and item["source_ts_ms"] == 1783360799900
                and item["server_ts_ms"] == 1783360800000
                and tag_from == publisher
            )
            with lock:
                received_typed[publisher] = item
                maybe_done()
        except Exception as exc:  # pragma: no cover - exercised on device
            with lock:
                errors[f"{publisher}:typed"] = str(exc)
                received_typed[publisher] = {
                    "body_case": None, "hex": None, "ok": False,
                }
                maybe_done()

    topic_filter = _gg_topic(run_id, "+", LANG)
    typed_topic_filter = _gg_typed_topic(run_id, "+", LANG)
    prov.subscribe(topic_filter, binary_handler, max_concurrency=1, max_messages=64)
    prov.subscribe(typed_topic_filter, typed_handler, max_concurrency=1, max_messages=64)
    print("READY", flush=True)
    with open(_gg_ready_path(run_id, ready_lang), "w", encoding="utf-8") as f:
        f.write(str(time.time()))
    try:
        ready_missing = _gg_wait_for_ready(run_id, ready_langs)
        time.sleep(subscribe_delay)
        if not ready_missing:
            binary_msg = (
                MessageBuilder.create("InteropBinary", "1.0")
                .with_payload(expected_bytes)
                .with_tags({"from": LANG})
                .build()
            )
            typed_msg = (
                MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                .with_southbound_signal_update(_typed_body(expected_hex))
                .with_tags({"from": LANG})
                .build()
            )
            for target in expected_langs:
                prov.publish(_gg_topic(run_id, LANG, target), binary_msg)
                prov.publish(_gg_typed_topic(run_id, LANG, target), typed_msg)
        done.wait(wait_secs)
        with lock:
            missing = [lang for lang in expected_langs if lang not in received]
            missing_typed = [
                lang for lang in expected_langs if lang not in received_typed
            ]
            ok = not ready_missing and not missing and not errors and all(
                received.get(lang, {}).get("ok") for lang in expected_langs
            ) and not missing_typed and all(
                received_typed.get(lang, {}).get("ok") for lang in expected_langs
            )
            result = {
                "ok": bool(ok),
                "lang": LANG,
                "run_id": run_id,
                "expected_hex": expected_hex.lower(),
                "ready_missing": ready_missing,
                "received": received,
                "received_typed": received_typed,
                "missing": missing,
                "missing_typed": missing_typed,
                "errors": errors,
            }
        result_path = f"/tmp/edgecommons_gg_ipc_binary_{LANG}_{run_id}.json"
        with open(result_path, "w", encoding="utf-8") as f:
            json.dump(result, f, sort_keys=True)
        print(json.dumps(result, sort_keys=True), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_uns_pub(identity_json, cls_token, channel=None):
    """Publish one envelope stamped with the given identity on the Uns-minted topic."""
    identity = MessageIdentity.from_dict(json.loads(identity_json))
    if identity is None:
        print(json.dumps({"ok": False, "error": "bad identity"}), flush=True)
        return 2
    cls = UnsClass.from_token(cls_token)
    if cls is None:
        print(json.dumps({"ok": False, "error": f"bad class '{cls_token}'"}), flush=True)
        return 2
    # The real topic builder, rootless (includeRoot=false) like the vectors/interop suite.
    topic = Uns(identity, False).topic(cls, channel if channel else None)
    prov = _provider("unspub")
    try:
        msg = (
            MessageBuilder.create("UnsInterop", "1.0")
            .with_payload({"from": LANG})
            .with_identity(identity)
            .build()
        )
        prov.publish(topic, msg)
        time.sleep(0.5)  # let the QoS-0 publish drain before disconnect
        print(json.dumps({"ok": True, "topic": topic, "envelope": msg.to_dict()}),
              flush=True)
        return 0
    finally:
        prov.disconnect()


def run_uns_sub(topic):
    """Receive one envelope on <topic> and print its parsed top-level identity."""
    prov = _provider("unssub")
    state = {}
    got = threading.Event()

    def handler(_t, m):
        state["msg"] = m
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(10):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        msg = state["msg"]
        identity = msg.get_identity()
        ok = identity is not None
        print(json.dumps({
            "ok": ok,
            "identity": identity.to_dict() if identity else None,
            "body": msg.get_body(),
        }), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_uns_guard(topic=None):
    """Attempt a reserved-class publish through the guarded public surface (must fail).

    ``topic`` selects the reserved target (D-U28): the instance-scoped
    ``ecv1/dev1/comp1/main/state`` (default) or the component-scoped
    ``ecv1/dev1/comp1/state`` — the guard must reject both.
    """
    from edgecommons.messaging.errors import ReservedTopicError
    from edgecommons.messaging.messaging_client import MessagingClient

    topic = topic or "ecv1/dev1/comp1/main/state"
    try:
        # The guard (§4.1) fires before the provider is dereferenced, so no broker
        # connection (and no MessagingClient.init) is needed to prove it.
        MessagingClient.publish_raw(topic, {"from": LANG})
    except ReservedTopicError as e:
        print(json.dumps({
            "error": "ReservedTopicError",
            "class": e.class_token,
            "topic": e.topic,
        }), flush=True)
        return 3
    print(json.dumps({"ok": True, "error": None}), flush=True)
    return 0


def _canonical_instances():
    """The canonical per-instance connectivity sample every language's node reports.

    Built through the public builder path (``of`` + ``with_state`` / ``with_attributes``),
    not the wide constructor.  The three elements pin the contract the interop matrix
    asserts: every optional member present (cam-01, including the OPEN ``attributes`` bag),
    ``connected=false`` with a richer own-vocabulary ``state`` (cam-02: BACKOFF is not
    FAILED), and the minimal element that must OMIT every optional member (cam-03).
    """
    return [
        InstanceConnectivity.of("cam-01", True, "rtsp://cam-01/stream")
        .with_state("ONLINE")
        .with_attributes({
            "capabilities": ["ptz", "snapshot"],
            "vendor": "acme",
            "retries": 0,
        }),
        InstanceConnectivity.of("cam-02", False, "connect timed out").with_state("BACKOFF"),
        InstanceConnectivity.of("cam-03", True),
    ]


def _interop_identity(component_token):
    """The component-scope wire identity of an interop component on the fixed
    ``interop-device`` thing. No instance token (D-U28): the library-owned `state`
    keepalive and the `status` command inbox are component-scoped, so a peer derives
    ``ecv1/interop-device/{component}/{class}`` from this identity."""
    return MessageIdentity.from_dict({
        "hier": [{"level": "device", "value": "interop-device"}],
        "path": "interop-device",
        "component": component_token,
    })


def _write_connectivity_runtime_config(component_token, heartbeat_enabled):
    """The real runtime config for the connectivity roles.

    ``heartbeat_enabled`` selects the surface under test: the PULL roles need only the
    command inbox, while the PUSH roles need the ``state`` keepalive running.
    """
    cfg = {
        "component": {"token": component_token},
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": HOST,
                "port": PORT,
                "clientId": f"interop-{LANG}-conn-runtime-{os.getpid()}",
            },
            "requestTimeoutSeconds": 4,
        },
        "heartbeat": {"enabled": bool(heartbeat_enabled), "intervalSecs": 2},
        "health": {"enabled": False},
    }
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(cfg, f)
        return f.name


def _run_connectivity_component(component_token, heartbeat_enabled):
    """Start a real component that reports the canonical connectivity sample, print READY
    and stay alive until terminated. One provider feeds both surfaces: the built-in
    ``status`` verb (pull) and the ``state`` keepalive's ``instances[]`` (push)."""
    path = _write_connectivity_runtime_config(component_token, heartbeat_enabled)
    gg = None
    try:
        gg = EdgeCommons(
            f"com.mbreissi.edgecommons.interop.{LANG}.ConnectivityComponent",
            _log_runtime_args(path),
        )
        gg.set_instance_connectivity_provider(_canonical_instances)
        if gg.get_commands() is None:
            raise RuntimeError("runtime did not expose command inbox")
        print("READY", flush=True)
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        pass
    finally:
        if gg is not None:
            gg.shutdown()
        try:
            os.unlink(path)
        except OSError:
            pass


def run_status_responder(component_token):
    """Serve the built-in ``status`` verb out of the registered connectivity provider."""
    _run_connectivity_component(component_token, heartbeat_enabled=False)


def run_state_instances_pub(component_token):
    """Push the same provider sample on the ``state`` keepalive (heartbeat enabled)."""
    _run_connectivity_component(component_token, heartbeat_enabled=True)


def run_status_request(component_token):
    """Pull <component>'s built-in ``status`` verb and print the reply's result body."""
    identity = _interop_identity(component_token)
    uns = Uns(identity, False)
    topic = uns.topic(UnsClass.CMD, "status")
    prov = _provider("statusreq")
    reply_topic = f"interop/status/reply/{LANG}/{uuid.uuid4().hex}"
    replies = []
    got = threading.Event()
    try:
        def on_reply(_topic, reply):
            replies.append(reply)
            got.set()

        prov.subscribe(reply_topic, on_reply)
        request = (
            MessageBuilder.create("status", "1.0")
            .with_command({})
            .with_reply_to(reply_topic)
            .with_tags({})
            .build()
        )
        correlation = request.get_correlation_id()
        prov.publish(topic, request)
        if not got.wait(15):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        reply = replies[0]
        body = reply.get_body()
        result = body.get("result") if isinstance(body, dict) else None
        ok = (
            reply.get_correlation_id() == correlation
            and isinstance(body, dict)
            and body.get("ok") is True
            and isinstance(result, dict)
            and result.get("status") == "RUNNING"
            and isinstance(result.get("uptimeSecs"), int)
        )
        print(json.dumps({"ok": bool(ok), "reply_body": result}), flush=True)
        return 0 if ok else 1
    finally:
        prov.disconnect()


def run_state_instances_sub(component_token):
    """Subscribe <component>'s reserved ``state`` topic (subscribing to a reserved class is
    allowed — only publishing to one is rejected) and report the first RUNNING keepalive
    that carries a non-empty ``instances[]``."""
    identity = _interop_identity(component_token)
    topic = Uns(identity, False).topic(UnsClass.STATE)
    prov = _provider("stateinstsub")
    state = {}
    got = threading.Event()

    def handler(_t, m):
        try:
            body = m.get_body()
            if not isinstance(body, dict) or body.get("status") != "RUNNING":
                return
            instances = body.get("instances")
            if not instances:
                return
            state["result"] = {
                "ok": True,
                "state_status": body.get("status"),
                "instances": instances,
            }
        except Exception as exc:  # pragma: no cover - exercised by subprocess harness
            state["result"] = {"ok": False, "error": str(exc)}
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(35):
            print(json.dumps({"ok": False, "error": "timeout", "topic": topic}), flush=True)
            return 1
        result = state["result"]
        print(json.dumps(result), flush=True)
        return 0 if result.get("ok") else 1
    finally:
        prov.disconnect()


if __name__ == "__main__":
    role = sys.argv[1]
    if role == "responder":
        run_responder(sys.argv[2])
    elif role == "request":
        sys.exit(run_request(sys.argv[2], sys.argv[3]))
    elif role == "deferred-responder":
        run_deferred_responder(sys.argv[2])
    elif role == "deferred-request":
        sys.exit(run_deferred_request(sys.argv[2], sys.argv[3]))
    elif role == "confirmed-sub":
        sys.exit(run_confirmed_sub(sys.argv[2], sys.argv[3]))
    elif role == "confirmed-pub":
        sys.exit(run_confirmed_pub(sys.argv[2], sys.argv[3]))
    elif role == "raw-sub":
        sys.exit(run_raw_sub(sys.argv[2], sys.argv[3]))
    elif role == "raw-pub":
        sys.exit(run_raw_pub(sys.argv[2], sys.argv[3]))
    elif role == "binary-sub":
        sys.exit(run_binary_sub(sys.argv[2], sys.argv[3]))
    elif role == "binary-pub":
        sys.exit(run_binary_pub(sys.argv[2], sys.argv[3]))
    elif role == "typed-sub":
        sys.exit(run_typed_sub(sys.argv[2], sys.argv[3]))
    elif role == "typed-pub":
        sys.exit(run_typed_pub(sys.argv[2], sys.argv[3]))
    elif role == "log-sub":
        sys.exit(run_log_sub(sys.argv[2], sys.argv[3]))
    elif role == "log-pub":
        sys.exit(run_log_pub(sys.argv[2]))
    elif role == "gg-log-matrix":
        sys.exit(run_gg_log_matrix(sys.argv[2], sys.argv[3], sys.argv[4] if len(sys.argv) > 4 else None))
    elif role == "gg-binary-matrix":
        sys.exit(run_gg_binary_matrix(sys.argv[2], sys.argv[3], sys.argv[4]))
    elif role == "gg-p1-matrix":
        sys.exit(run_gg_p1_matrix(sys.argv[2], sys.argv[3], sys.argv[4] if len(sys.argv) > 4 else None))
    elif role == "uns-pub":
        sys.exit(run_uns_pub(sys.argv[2], sys.argv[3],
                             sys.argv[4] if len(sys.argv) > 4 else None))
    elif role == "uns-sub":
        sys.exit(run_uns_sub(sys.argv[2]))
    elif role == "uns-guard":
        sys.exit(run_uns_guard(sys.argv[2] if len(sys.argv) > 2 else None))
    elif role == "status-responder":
        run_status_responder(sys.argv[2])
    elif role == "status-request":
        sys.exit(run_status_request(sys.argv[2]))
    elif role == "state-instances-pub":
        run_state_instances_pub(sys.argv[2])
    elif role == "state-instances-sub":
        sys.exit(run_state_instances_sub(sys.argv[2]))
    else:
        sys.stderr.write(f"unknown role: {role}\n")
        sys.exit(2)
