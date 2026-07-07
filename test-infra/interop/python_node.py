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
"""
import json
import os
import base64
import sys
import tempfile
import threading
import time

from edgecommons.messaging.messaging_config import MessagingConfiguration
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider
from edgecommons.messaging.message import _binary_marker
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.identity import MessageIdentity
from edgecommons.uns import Uns, UnsClass

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


def run_uns_guard():
    """Attempt a reserved-class publish through the guarded public surface (must fail)."""
    from edgecommons.messaging.errors import ReservedTopicError
    from edgecommons.messaging.messaging_client import MessagingClient

    topic = "ecv1/dev1/comp1/main/state"
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


if __name__ == "__main__":
    role = sys.argv[1]
    if role == "responder":
        run_responder(sys.argv[2])
    elif role == "request":
        sys.exit(run_request(sys.argv[2], sys.argv[3]))
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
    elif role == "gg-binary-matrix":
        sys.exit(run_gg_binary_matrix(sys.argv[2], sys.argv[3], sys.argv[4]))
    elif role == "uns-pub":
        sys.exit(run_uns_pub(sys.argv[2], sys.argv[3],
                             sys.argv[4] if len(sys.argv) > 4 else None))
    elif role == "uns-sub":
        sys.exit(run_uns_sub(sys.argv[2]))
    elif role == "uns-guard":
        sys.exit(run_uns_guard())
    else:
        sys.stderr.write(f"unknown role: {role}\n")
        sys.exit(2)
