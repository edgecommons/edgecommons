"""Internal hierarchical-config helpers.

Direct providers supply one effective document. CONFIG_COMPONENT supplies ordered
lineage layers, and only ``layers[].config`` participates in the merge.
"""

import copy
import hashlib
import json
import logging
from dataclasses import dataclass
from typing import Any, Callable, Dict, Iterable, List, Optional

logger = logging.getLogger("HierarchicalConfig")


class HierarchicalConfigError(ValueError):
    """Raised when hierarchical config cannot produce an effective config."""

    def __init__(self, code: str, message: str):
        self.code = code
        super().__init__(message)


@dataclass(frozen=True)
class ParsedConfigComponentPayload:
    catalog_version: str
    component: str
    layers: List[Dict[str, Any]]
    configs: List[Dict[str, Any]]


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
    """Deep-merge JSON object layers with later layers winning.

    Objects merge recursively. Arrays, scalars, and ``null`` replace the earlier
    value. Input layers are never mutated.
    """
    result: Dict[str, Any] = {}
    for layer in layers:
        if layer is None:
            continue
        if not isinstance(layer, dict):
            raise HierarchicalConfigError("CONFIG_LAYER_INVALID", "config layer must be a JSON object")
        result = _merge_value(result, layer, "$", warn)
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


def parse_config_component_payload(
    payload: Any,
    request_component: Optional[str] = None,
) -> ParsedConfigComponentPayload:
    """Parse and validate a CONFIG_COMPONENT lineage bundle.

    Structured error bodies surface their embedded error code. Old
    ``{base, component}`` bundles and legacy component-only documents are
    rejected as ``LINEAGE_BUNDLE_INVALID`` because the replacement contract is
    lineage-only.
    """
    if isinstance(payload, str):
        try:
            payload = json.loads(payload)
        except json.JSONDecodeError as e:
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT payload is malformed JSON: {e}",
            ) from e
    if not isinstance(payload, dict):
        raise HierarchicalConfigError(
            "LINEAGE_BUNDLE_INVALID",
            "CONFIG_COMPONENT payload must be a JSON object",
        )
    if payload.get("ok") is False and isinstance(payload.get("error"), dict):
        error = payload["error"]
        code = error.get("code") if isinstance(error.get("code"), str) else "CONFIG_COMPONENT_ERROR"
        message = error.get("message") if isinstance(error.get("message"), str) else code
        raise HierarchicalConfigError(code, message)

    if payload.get("lineageVersion") != 1:
        raise HierarchicalConfigError(
            "LINEAGE_BUNDLE_INVALID",
            "CONFIG_COMPONENT payload must declare lineageVersion 1",
        )
    catalog_version = payload.get("catalogVersion")
    if not isinstance(catalog_version, str) or catalog_version == "":
        raise HierarchicalConfigError(
            "LINEAGE_BUNDLE_INVALID",
            "CONFIG_COMPONENT payload must contain a non-empty catalogVersion",
        )
    component = payload.get("component")
    if not isinstance(component, str) or component == "":
        raise HierarchicalConfigError(
            "LINEAGE_BUNDLE_INVALID",
            "CONFIG_COMPONENT payload must contain a non-empty component",
        )
    if request_component is not None and component != request_component:
        raise HierarchicalConfigError(
            "LINEAGE_BUNDLE_INVALID",
            f"CONFIG_COMPONENT payload component '{component}' does not match requested component '{request_component}'",
        )
    layers = payload.get("layers")
    if not isinstance(layers, list) or len(layers) == 0:
        raise HierarchicalConfigError(
            "LINEAGE_BUNDLE_INVALID",
            "CONFIG_COMPONENT payload must contain a non-empty layers array",
        )

    validated_layers: List[Dict[str, Any]] = []
    configs: List[Dict[str, Any]] = []
    for index, layer in enumerate(layers):
        if not isinstance(layer, dict):
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT layer at index {index} must be a JSON object",
            )
        layer_id = layer.get("id")
        kind = layer.get("kind")
        if not isinstance(layer_id, str) or layer_id == "":
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT layer at index {index} must contain a non-empty id",
            )
        if kind not in ("scope", "component"):
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT layer '{layer_id}' must declare kind 'scope' or 'component'",
            )
        if kind == "component":
            if index != len(layers) - 1:
                raise HierarchicalConfigError(
                    "LINEAGE_BUNDLE_INVALID",
                    "CONFIG_COMPONENT component layer must be final",
                )
            if layer.get("component") != component:
                raise HierarchicalConfigError(
                    "LINEAGE_BUNDLE_INVALID",
                    f"CONFIG_COMPONENT component layer '{layer_id}' does not match bundle component '{component}'",
                )
        elif index == len(layers) - 1:
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                "CONFIG_COMPONENT final layer must be kind 'component'",
            )
        elif not isinstance(layer.get("scope"), dict):
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT scope layer '{layer_id}' must contain object scope",
            )
        config = layer.get("config")
        if not isinstance(config, dict):
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT layer '{layer_id}' config must be a JSON object",
            )
        scope = layer.get("scope")
        if scope is not None and not isinstance(scope, dict):
            raise HierarchicalConfigError(
                "LINEAGE_BUNDLE_INVALID",
                f"CONFIG_COMPONENT layer '{layer_id}' scope must be a JSON object when present",
            )
        validated_layers.append(copy.deepcopy(layer))
        configs.append(config)

    _validate_scope_ownership(validated_layers)
    _validate_identity_ownership(validated_layers)
    return ParsedConfigComponentPayload(catalog_version, component, validated_layers, configs)


def _validate_scope_ownership(layers: Iterable[Dict[str, Any]]) -> None:
    owned: Dict[str, Any] = {}
    for layer in layers:
        scope = layer.get("scope")
        if not isinstance(scope, dict):
            continue
        for key, value in scope.items():
            if key in owned and owned[key] != value:
                raise HierarchicalConfigError(
                    "LINEAGE_SCOPE_CONFLICT",
                    f"lineage scope key '{key}' changed from '{owned[key]}' to '{value}'",
                )
            owned[key] = value


def _validate_identity_ownership(layers: Iterable[Dict[str, Any]]) -> None:
    owned: Dict[str, Any] = {}
    for layer in layers:
        config = layer.get("config")
        identity = config.get("identity") if isinstance(config, dict) else None
        if not isinstance(identity, dict):
            continue
        for key, value in identity.items():
            if key in owned and owned[key] != value:
                raise HierarchicalConfigError(
                    "LINEAGE_IDENTITY_CONFLICT",
                    f"lineage identity key '{key}' changed from '{owned[key]}' to '{value}'",
                )
            owned[key] = value


def derive_catalog_version(catalog: Dict[str, Any], source_uri: str) -> str:
    """Small helper for Python-owned catalog-vector client error cases."""
    encoded = json.dumps(catalog, sort_keys=True, separators=(",", ":")).encode("utf-8")
    digest = hashlib.sha256(encoded).hexdigest()
    return f"sha256:{digest}" if not source_uri else f"{source_uri}#sha256:{digest}"
