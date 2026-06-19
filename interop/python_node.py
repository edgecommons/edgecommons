"""Cross-language interop node (Python) for ggcommons.

Two roles, talking to the shared local MQTT broker (localhost:1883) in STANDALONE
local-only mode:

  python_node.py responder <request_topic>
      Subscribe to <request_topic>; reply to each request with
      {"echo": <request body>, "responder": "python"} (the lib copies the
      correlation id and publishes to the request's reply_to). Prints "READY"
      once subscribed, then runs until killed.

  python_node.py request <request_topic> <token>
      Send a request {"token": <token>, "from": "python"} and wait for the reply.
      Prints one JSON line and exits 0 on a correlated, well-formed reply, else 1.
"""
import json
import os
import sys
import tempfile
import time

from ggcommons.messaging.messaging_config import MessagingConfiguration
from ggcommons.messaging.providers.standalone_provider import StandaloneProvider
from ggcommons.messaging.message_builder import MessageBuilder

LANG = "python"
HOST = os.environ.get("GGCOMMONS_IT_MQTT_HOST", "localhost")
PORT = int(os.environ.get("GGCOMMONS_IT_MQTT_PORT", "1883"))


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
            .with_payload({"token": token, "from": LANG})
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


if __name__ == "__main__":
    role = sys.argv[1]
    if role == "responder":
        run_responder(sys.argv[2])
    elif role == "request":
        sys.exit(run_request(sys.argv[2], sys.argv[3]))
    else:
        sys.stderr.write(f"unknown role: {role}\n")
        sys.exit(2)
