"""Internal split-config helpers.

This module owns the raw-layer contract: provider documents may contain the
top-level control fields ``extends`` and ``sharedConfig``; effective config
snapshots never do.
"""

import copy
import hashlib
import json
import logging
import os
from dataclasses import dataclass
from typing import Any, Callable, Dict, Iterable, Optional

logger = logging.getLogger("SplitConfig")

CONTROL_FIELDS = {"extends", "sharedConfig"}
SHARED_CONFIG_ENV = "EDGECOMMONS_SHARED_CONFIG"
SHARED_COMPONENT_ENV = "EDGECOMMONS_SHARED_COMPONENT"
DEFAULT_FILE_SHARED_CONFIG = "/etc/edgecommons/shared.json"
DEFAULT_SHARED_COMPONENT = "com.mbreissi.edgecommons.EdgeCommonsSharedConfig"
SHARED_COMPONENT_KEY = "SharedComponentConfig"
SHARED_SHADOW_NAME = "edgecommons-shared"
SHADOW_COMPONENT_CONFIG_KEY = "ComponentConfig"


class SplitConfigError(ValueError):
    """Raised when raw split-config layers cannot produce an effective config."""

    def __init__(self, code: str, message: str):
        self.code = code
        super().__init__(message)


@dataclass(frozen=True)
class BaseLayer:
    value: Optional[Dict[str, Any]]
    source: Optional[str] = None


@dataclass(frozen=True)
class ParsedConfigComponentPayload:
    component: Dict[str, Any]
    base: Optional[Dict[str, Any]]
    base_present: bool


def strip_control_fields(layer: Dict[str, Any]) -> Dict[str, Any]:
    """Return a deep copy of ``layer`` without raw top-level control fields."""
    stripped = copy.deepcopy(layer)
    for key in CONTROL_FIELDS:
        stripped.pop(key, None)
    return stripped


def _json_type(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, dict):
        return "object"
    if isinstance(value, list):
        return "array"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, (int, float)):
        return "number"
    if isinstance(value, str):
        return "string"
    return type(value).__name__


def _should_warn_type_conflict(left: Any, right: Any) -> bool:
    if isinstance(left, list) or isinstance(right, list):
        return False
    return left is not None and right is not None and _json_type(left) != _json_type(right)


def deep_merge_layers(
    layers: Iterable[Optional[Dict[str, Any]]],
    warn: Optional[Callable[[str, str, str], None]] = None,
) -> Dict[str, Any]:
    """Deep-merge raw JSON object layers with later layers winning.

    Objects merge recursively; arrays, scalars, and ``null`` replace. Input layers
    are never mutated. Raw control fields are stripped from every layer before
    merging.
    """
    result: Dict[str, Any] = {}
    for layer in layers:
        if layer is None:
            continue
        if not isinstance(layer, dict):
            raise SplitConfigError("CONFIG_LAYER_INVALID", "config layer must be a JSON object")
        result = _merge_value(result, strip_control_fields(layer), "$", warn)
    return result


def _merge_value(left: Any, right: Any, path: str, warn: Optional[Callable[[str, str, str], None]]) -> Any:
    if isinstance(left, dict) and isinstance(right, dict):
        merged = copy.deepcopy(left)
        for key, value in right.items():
            child_path = f"{path}.{key}"
            if key in merged:
                merged[key] = _merge_value(merged[key], value, child_path, warn)
            else:
                merged[key] = copy.deepcopy(value)
        return merged
    if _should_warn_type_conflict(left, right) and warn is not None:
        warn(path, _json_type(left), _json_type(right))
    return copy.deepcopy(right)


def shared_config_enabled(component_layer: Dict[str, Any], no_shared_config: bool) -> bool:
    if no_shared_config:
        return False
    value = component_layer.get("sharedConfig")
    if value is None:
        return True
    if isinstance(value, bool):
        return value
    raise SplitConfigError(
        "SHARED_CONFIG_CONTROL_INVALID",
        "top-level sharedConfig must be a boolean when present",
    )


def validate_component_layer(layer: Dict[str, Any]) -> None:
    if not isinstance(layer, dict):
        raise SplitConfigError("CONFIG_LAYER_INVALID", "component config layer must be a JSON object")
    if "sharedConfig" in layer and not isinstance(layer["sharedConfig"], bool):
        raise SplitConfigError(
            "SHARED_CONFIG_CONTROL_INVALID",
            "top-level sharedConfig must be a boolean when present",
        )
    if "extends" in layer:
        value = layer["extends"]
        if not isinstance(value, str) or value == "":
            raise SplitConfigError(
                "EXTENDS_INVALID",
                "top-level extends must be a non-empty string when present",
            )


def validate_base_layer(layer: Optional[Dict[str, Any]]) -> None:
    if layer is None:
        return
    if not isinstance(layer, dict):
        raise SplitConfigError("SHARED_CONFIG_INVALID", "shared config layer must be a JSON object")
    if "extends" in layer:
        raise SplitConfigError(
            "N_LAYER_INHERITANCE_NOT_IMPLEMENTED",
            "shared config layer contains extends; N-layer inheritance is not implemented",
        )


def validate_file_extends(component_layer: Dict[str, Any]) -> Optional[str]:
    if "extends" not in component_layer:
        return None
    value = component_layer["extends"]
    if not isinstance(value, str) or value == "":
        raise SplitConfigError(
            "EXTENDS_INVALID",
            "top-level extends must be a non-empty string when present",
        )
    return value


def read_json_object(path: str, source_label: str) -> Dict[str, Any]:
    try:
        with open(path) as f:
            value = json.load(f)
    except Exception as e:
        raise SplitConfigError(
            "SHARED_CONFIG_UNAVAILABLE",
            f"unable to read shared config from {source_label}: {e}",
        ) from e
    if not isinstance(value, dict):
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"shared config from {source_label} must be a JSON object",
        )
    return value


def resolve_file_base(component_path: str, component_layer: Dict[str, Any], env=None) -> BaseLayer:
    env = os.environ if env is None else env
    extends = validate_file_extends(component_layer)
    if extends is not None:
        path = extends if os.path.isabs(extends) else os.path.join(os.path.dirname(os.path.abspath(component_path)), extends)
        return BaseLayer(read_json_object(path, path), os.path.abspath(path))
    env_value = env.get(SHARED_CONFIG_ENV)
    if env_value:
        return BaseLayer(read_json_object(env_value, env_value), os.path.abspath(env_value))
    if os.path.exists(DEFAULT_FILE_SHARED_CONFIG):
        return BaseLayer(
            read_json_object(DEFAULT_FILE_SHARED_CONFIG, DEFAULT_FILE_SHARED_CONFIG),
            os.path.abspath(DEFAULT_FILE_SHARED_CONFIG),
        )
    return BaseLayer(None, None)


def resolve_configmap_base(
    mount_dir: str,
    component_path: str,
    component_layer: Dict[str, Any],
    env=None,
) -> BaseLayer:
    from edgecommons.parameters.source import is_projection_artifact

    env = os.environ if env is None else env
    extends = validate_file_extends(component_layer)
    if extends is not None:
        if is_projection_artifact(os.path.basename(extends)):
            raise SplitConfigError(
                "EXTENDS_INVALID",
                "ConfigMap shared config key must not be a kubelet projection artifact",
            )
        path = extends if os.path.isabs(extends) else os.path.join(os.path.dirname(os.path.abspath(component_path)), extends)
        return BaseLayer(read_json_object(path, path), os.path.abspath(path))
    env_value = env.get(SHARED_CONFIG_ENV)
    if env_value:
        if is_projection_artifact(os.path.basename(env_value)):
            raise SplitConfigError(
                "EXTENDS_INVALID",
                "ConfigMap shared config key must not be a kubelet projection artifact",
            )
        return BaseLayer(read_json_object(env_value, env_value), os.path.abspath(env_value))
    path = os.path.join(mount_dir, "shared.json")
    if os.path.exists(path):
        return BaseLayer(read_json_object(path, path), os.path.abspath(path))
    return BaseLayer(None, None)


def resolve_env_base(env=None) -> BaseLayer:
    env = os.environ if env is None else env
    value = env.get(SHARED_CONFIG_ENV)
    if not value:
        return BaseLayer(None, None)
    if value.startswith("@"):
        path = value[1:]
        if not path:
            raise SplitConfigError("SHARED_CONFIG_UNAVAILABLE", "EDGECOMMONS_SHARED_CONFIG @path is empty")
        return BaseLayer(read_json_object(path, path), os.path.abspath(path))
    try:
        parsed = json.loads(value)
    except json.JSONDecodeError as e:
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"EDGECOMMONS_SHARED_CONFIG inline JSON is malformed: {e}",
        ) from e
    if not isinstance(parsed, dict):
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            "EDGECOMMONS_SHARED_CONFIG inline JSON must be an object",
        )
    return BaseLayer(parsed, "env:EDGECOMMONS_SHARED_CONFIG")


def resolve_greengrass_base(env=None, client_factory=None) -> BaseLayer:
    env = os.environ if env is None else env
    explicit = SHARED_COMPONENT_ENV in env and bool(env.get(SHARED_COMPONENT_ENV))
    component = env.get(SHARED_COMPONENT_ENV) or DEFAULT_SHARED_COMPONENT
    if client_factory is None:
        from awsiot.greengrasscoreipc.clientv2 import GreengrassCoreIPCClientV2
        client_factory = GreengrassCoreIPCClientV2
    client = client_factory()
    try:
        response = client.get_configuration(component_name=component)
        value = getattr(response, "value", None)
        if value is None or SHARED_COMPONENT_KEY not in value:
            if explicit:
                raise SplitConfigError(
                    "SHARED_CONFIG_UNAVAILABLE",
                    f"shared Greengrass config '{component}:{SHARED_COMPONENT_KEY}' is unavailable",
                )
            return BaseLayer(None, None)
        base = value.get(SHARED_COMPONENT_KEY)
        if not isinstance(base, dict):
            raise SplitConfigError(
                "SHARED_CONFIG_INVALID",
                f"shared Greengrass config '{component}:{SHARED_COMPONENT_KEY}' must be an object",
            )
        return BaseLayer(base, f"gg-config:{component}:{SHARED_COMPONENT_KEY}")
    except SplitConfigError:
        raise
    except Exception as e:
        if explicit:
            raise SplitConfigError(
                "SHARED_CONFIG_UNAVAILABLE",
                f"shared Greengrass config '{component}:{SHARED_COMPONENT_KEY}' is unavailable: {e}",
            ) from e
        return BaseLayer(None, None)
    finally:
        close = getattr(client, "close", None)
        if close is not None:
            close()


def parse_shadow_base_payload(payload: bytes) -> Optional[Dict[str, Any]]:
    if payload is None or len(payload) == 0:
        return None
    try:
        payload_json = json.loads(str(payload, "utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as e:
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"shared shadow {SHARED_SHADOW_NAME} document is malformed JSON: {e}",
        ) from e
    if not isinstance(payload_json, dict):
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"shared shadow {SHARED_SHADOW_NAME} document must be an object",
        )
    state_doc = payload_json.get("state", {})
    if not isinstance(state_doc, dict):
        return None
    raw = None
    desired = state_doc.get("desired")
    reported = state_doc.get("reported")
    if isinstance(desired, dict):
        raw = desired.get(SHADOW_COMPONENT_CONFIG_KEY)
    if raw is None and isinstance(reported, dict):
        raw = reported.get(SHADOW_COMPONENT_CONFIG_KEY)
    if raw is None:
        return None
    if not isinstance(raw, str):
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"shared shadow {SHARED_SHADOW_NAME} {SHADOW_COMPONENT_CONFIG_KEY} must be a stringified object",
        )
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError as e:
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"shared shadow {SHARED_SHADOW_NAME} {SHADOW_COMPONENT_CONFIG_KEY} is malformed JSON: {e}",
        ) from e
    if not isinstance(parsed, dict):
        raise SplitConfigError(
            "SHARED_CONFIG_INVALID",
            f"shared shadow {SHARED_SHADOW_NAME} {SHADOW_COMPONENT_CONFIG_KEY} must be an object",
        )
    return parsed


def resolve_shadow_base(ipc_client, thing_name: str) -> BaseLayer:
    try:
        response = ipc_client.get_thing_shadow(
            thing_name=thing_name,
            shadow_name=SHARED_SHADOW_NAME,
        )
        base = parse_shadow_base_payload(getattr(response, "payload", None))
        if base is None:
            return BaseLayer(None, None)
        return BaseLayer(base, f"shadow:{SHARED_SHADOW_NAME}:{SHADOW_COMPONENT_CONFIG_KEY}")
    except SplitConfigError:
        raise
    except Exception:
        return BaseLayer(None, None)


def parse_config_component_payload(payload: Any) -> ParsedConfigComponentPayload:
    if isinstance(payload, str):
        try:
            payload = json.loads(payload)
        except json.JSONDecodeError as e:
            raise SplitConfigError(
                "CONFIG_COMPONENT_BUNDLE_INVALID",
                f"CONFIG_COMPONENT payload is malformed JSON: {e}",
            ) from e
    if not isinstance(payload, dict):
        raise SplitConfigError(
            "CONFIG_COMPONENT_BUNDLE_INVALID",
            "CONFIG_COMPONENT payload must be a JSON object",
        )
    if payload.get("ok") is False and isinstance(payload.get("error"), dict):
        error = payload["error"]
        code = error.get("code") if isinstance(error.get("code"), str) else "CONFIG_COMPONENT_ERROR"
        message = error.get("message") if isinstance(error.get("message"), str) else code
        raise SplitConfigError(code, message)
    if "base" not in payload:
        return ParsedConfigComponentPayload(payload, None, False)
    if "component" not in payload or not isinstance(payload.get("component"), dict):
        raise SplitConfigError(
            "CONFIG_COMPONENT_BUNDLE_INVALID",
            "CONFIG_COMPONENT layer bundle must contain an object 'component' field",
        )
    base = payload.get("base")
    if base is not None and not isinstance(base, dict):
        raise SplitConfigError(
            "CONFIG_COMPONENT_BUNDLE_INVALID",
            "CONFIG_COMPONENT layer bundle 'base' field must be an object or null",
        )
    return ParsedConfigComponentPayload(payload["component"], base, True)


def derive_catalog_version(catalog: Dict[str, Any], source_uri: str) -> str:
    """Small helper for Python-owned catalog-vector client error cases."""
    encoded = json.dumps(catalog, sort_keys=True, separators=(",", ":")).encode("utf-8")
    digest = hashlib.sha256(encoded).hexdigest()
    return f"sha256:{digest}" if not source_uri else f"{source_uri}#sha256:{digest}"
