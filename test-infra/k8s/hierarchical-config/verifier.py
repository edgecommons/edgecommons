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


DEVICE = os.environ.get("EDGECOMMONS_TEST_DEVICE", "edgecommons-k8s-line-7")
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
EXPECTED_LINEAGE_PREFIX = [
    "enterprise/acme",
    "site/integration-lab",
    "zone/k8s-zone",
    "line/line-7",
]


def provider():
    cfg = {
        "messaging": {
            "local": {
                "type": "mqtt",
                "host": HOST,
                "port": PORT,
                "clientId": f"hierarchical-config-verifier-{os.getpid()}",
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
    return StandaloneProvider(config, "hierarchical-config-verifier")


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


def merge_value(left, right):
    if isinstance(left, dict) and isinstance(right, dict):
        merged = dict(left)
        for key, value in right.items():
            merged[key] = merge_value(merged[key], value) if key in merged else value
        return merged
    return right


def merge_layers(layers):
    effective = {}
    for layer in layers:
        effective = merge_value(effective, layer["config"])
    return effective


def verify_lineage(token, body, expected_marker):
    component_token = COMPONENT_TOKENS.get(token, token)
    require(isinstance(body, dict), f"{token}: reply body is not an object")
    require("base" not in body, f"{token}: old base layer appeared in lineage body")
    require(body.get("lineageVersion") == 1, f"{token}: unsupported lineageVersion {body.get('lineageVersion')}")
    require(body.get("component") == token, f"{token}: wire component mismatch {body.get('component')}")

    layers = body.get("layers")
    require(isinstance(layers, list) and layers, f"{token}: missing non-empty layers")
    ids = [layer.get("id") for layer in layers]
    require(ids[:-1] == EXPECTED_LINEAGE_PREFIX, f"{token}: wrong lineage prefix {ids}")
    require(ids[-1] == f"component/{token}", f"{token}: wrong component lineage id {ids[-1]}")

    scope = {}
    identity_owner = {}
    for layer in layers:
        require(isinstance(layer, dict), f"{token}: layer is not an object")
        require(isinstance(layer.get("config"), dict), f"{token}: layer config is not an object")
        layer_scope = layer.get("scope", {})
        require(isinstance(layer_scope, dict), f"{token}: layer scope is not an object")
        for key, value in layer_scope.items():
            if key in scope:
                require(scope[key] == value, f"{token}: scope conflict for {key}")
            scope[key] = value
        identity = layer.get("config", {}).get("identity", {})
        if identity is not None:
            require(isinstance(identity, dict), f"{token}: identity is not an object")
            for key, value in identity.items():
                if key in identity_owner:
                    require(identity_owner[key] == value, f"{token}: identity conflict for {key}")
                identity_owner[key] = value

    effective = merge_layers(layers)
    require(
        effective.get("hierarchy", {}).get("levels")
        == ["enterprise", "site", "zone", "line", "device"],
        f"{token}: hierarchy levels were not inherited",
    )
    require(effective.get("identity", {}).get("enterprise") == "acme", f"{token}: missing enterprise identity")
    require(effective.get("identity", {}).get("site") == "integration-lab", f"{token}: missing site identity")
    require(effective.get("identity", {}).get("zone") == "k8s-zone", f"{token}: missing zone identity")
    require(effective.get("identity", {}).get("line") == "line-7", f"{token}: missing line identity")
    require(effective.get("tags", {}).get("lineageMarker") == expected_marker, f"{token}: wrong lineage marker")
    require(effective.get("component", {}).get("token") == component_token, f"{token}: wrong component token")
    require(
        effective.get("component", {}).get("global", {}).get("unique_token") == token,
        f"{token}: missing unique component config",
    )
    return {
        "catalogVersion": body.get("catalogVersion"),
        "lineageIds": ids,
        "lineageMarker": effective.get("tags", {}).get("lineageMarker"),
        "componentToken": effective.get("component", {}).get("token"),
        "publishInterval": effective.get("component", {}).get("global", {}).get("publish_interval"),
        "uniqueToken": effective.get("component", {}).get("global", {}).get("unique_token"),
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
            initial[token] = verify_lineage(token, body, "k8s-hierarchical-initial")

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
            pushed[token] = verify_lineage(token, pushes[token], "k8s-hierarchical-updated")
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
