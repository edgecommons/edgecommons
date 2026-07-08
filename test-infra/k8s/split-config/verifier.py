import json
import os
import sys
import tempfile
import threading
import time

from edgecommons.messaging.errors import RequestTimeoutError
from edgecommons.messaging.message import Message
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.messaging.messaging_config import MessagingConfiguration
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider


DEVICE = os.environ.get("EDGECOMMONS_TEST_DEVICE", "edgecommons-k8s-split")
HOST = os.environ.get("EDGECOMMONS_IT_MQTT_HOST", "edgecommons-emqx")
PORT = int(os.environ.get("EDGECOMMONS_IT_MQTT_PORT", "1883"))
TOKENS = [
    token.strip()
    for token in os.environ.get(
        "EDGECOMMONS_TEST_TOKENS",
        "JavaComponentSkeleton,PythonComponentSkeleton,RustComponentSkeleton,TsComponentSkeleton",
    ).split(",")
    if token.strip()
]
COMPONENT_TOKENS = {
    "JavaComponentSkeleton": "java-component-skeleton",
    "PythonComponentSkeleton": "python-component-skeleton",
    "RustComponentSkeleton": "rust-component-skeleton",
    "TsComponentSkeleton": "ts-component-skeleton",
}
CATALOG_PATH = os.environ.get("EDGECOMMONS_UPDATE_CATALOG", "/etc/edgecommons/updated-catalog.json")


def provider():
    cfg = {
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": HOST,
                "port": PORT,
                "clientId": f"split-config-verifier-{os.getpid()}",
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
    return StandaloneProvider(config, "split-config-verifier")


def request_json(prov, topic, name, body, timeout=20):
    request = MessageBuilder.create(name, "1.0").with_payload(body).with_tags({}).build()
    correlation_id = request.get_correlation_id()
    try:
        done, reply = prov.request(topic, request).get(timeout)
    except RequestTimeoutError as exc:
        raise RuntimeError(f"request to {topic} timed out: {exc}") from exc
    if not done or reply is None:
        raise RuntimeError(f"request to {topic} timed out")
    if reply.get_correlation_id() != correlation_id:
        raise RuntimeError(f"reply correlation mismatch on {topic}")
    reply_body = reply.get_body()
    if isinstance(reply_body, str):
        reply_body = json.loads(reply_body)
    return reply_body


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def verify_bundle(token, body, expected_marker):
    component_token = COMPONENT_TOKENS.get(token, token)
    require(isinstance(body, dict), f"{token}: reply body is not an object")
    require("base" in body, f"{token}: missing base layer")
    require("component" in body, f"{token}: missing component layer")
    base = body["base"]
    component = body["component"]
    require(isinstance(base, dict), f"{token}: base layer is not an object")
    require(isinstance(component, dict), f"{token}: component layer is not an object")
    require(base.get("tags", {}).get("sharedLayer") == expected_marker, f"{token}: wrong shared marker")
    require(component.get("component", {}).get("token") == component_token, f"{token}: wrong component token")
    require(
        component.get("component", {}).get("global", {}).get("unique_token") == token,
        f"{token}: missing unique component config",
    )
    return {
        "sharedLayer": base.get("tags", {}).get("sharedLayer"),
        "componentToken": component.get("component", {}).get("token"),
        "publishInterval": component.get("component", {}).get("global", {}).get("publish_interval"),
        "uniqueToken": component.get("component", {}).get("global", {}).get("unique_token"),
    }


def main():
    get_topic = f"ecv1/{DEVICE}/config/main/cmd/get-configuration"
    update_topic = f"ecv1/{DEVICE}/config/main/cmd/update-catalog"
    push_prefix = f"ecv1/{DEVICE}"
    prov = provider()
    pushes = {}
    got_push = threading.Event()

    try:
        initial = {}
        for token in TOKENS:
            body = request_json(prov, get_topic, "GetConfiguration", {"component": token})
            initial[token] = verify_bundle(token, body, "k8s-split-initial")

        def make_handler(token):
            def handler(_topic, message: Message):
                body = message.get_body()
                if isinstance(body, str):
                    body = json.loads(body)
                pushes[token] = body
                if len(pushes) >= len(TOKENS):
                    got_push.set()
            return handler

        for token in TOKENS:
            prov.subscribe(f"{push_prefix}/{token}/main/cmd/set-config", make_handler(token))

        with open(CATALOG_PATH, "r", encoding="utf-8") as f:
            catalog = json.load(f)
        version = catalog["version"]
        ack = request_json(
            prov,
            update_topic,
            "UpdateCatalog",
            {"version": version, "catalog": catalog},
        )
        require(ack.get("ok") is True, f"update ack was not ok: {ack}")
        got_push.wait(20)
        require(len(pushes) == len(TOKENS), f"expected {len(TOKENS)} pushes, got {len(pushes)}")

        pushed = {}
        for token in TOKENS:
            pushed[token] = verify_bundle(token, pushes[token], "k8s-split-updated")
            require(pushed[token]["publishInterval"] == 1, f"{token}: updated publish_interval not applied")

        result = {
            "ok": True,
            "device": DEVICE,
            "getTopic": get_topic,
            "updateTopic": update_topic,
            "tokens": TOKENS,
            "initial": initial,
            "ack": ack,
            "pushed": pushed,
        }
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception as exc:
        print(json.dumps({"ok": False, "error": str(exc), "pushesSeen": sorted(pushes)}), file=sys.stderr)
        return 1
    finally:
        prov.disconnect()
        time.sleep(0.2)


if __name__ == "__main__":
    sys.exit(main())
