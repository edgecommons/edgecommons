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
import sys
import tempfile
import threading
import time

from edgecommons.messaging.messaging_config import MessagingConfiguration
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider
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
        state["raw"] = m.get_raw()
        got.set()

    prov.subscribe(topic, handler)
    print("READY", flush=True)
    try:
        if not got.wait(10):
            print(json.dumps({"ok": False, "error": "timeout"}), flush=True)
            return 1
        raw = state["raw"]
        is_raw = raw is not None
        raw_token = raw.get("token") if isinstance(raw, dict) else None
        ok = is_raw and raw_token == token
        print(json.dumps({"ok": bool(ok), "is_raw": is_raw, "raw_token": raw_token}), flush=True)
        return 0 if ok else 1
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
