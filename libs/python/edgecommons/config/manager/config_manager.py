import logging
import os
import re
import time
import copy
import threading
from dataclasses import dataclass
from typing import Dict, Any, Mapping, Optional, Tuple

from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.platform.resolver import (
    ENV_K8S_NODE_NAME,
    ENV_K8S_POD_NAME,
    ENV_K8S_POD_NAMESPACE,
    profile_logging_format,
)
from edgecommons.config.heartbeat_config import HeartbeatConfiguration
from edgecommons.config.health_config import HealthConfiguration
from edgecommons.config.metric_config import MetricConfiguration
from edgecommons.config.tag_config import TagConfiguration
from edgecommons.config.enhanced_logging_config import EnhancedLoggingConfiguration
from edgecommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from edgecommons.config.manager.hierarchical_config import (
    deep_merge_layers,
    parse_config_component_payload,
    HierarchicalConfigError,
)
from edgecommons.config.candidate_validation import (
    DEFAULT_CANDIDATE_VALIDATION_TIMEOUT_SECS,
    ConfigurationCandidateRejected,
    ConfigurationCandidateValidator,
    ConfigurationValidationError,
    ConfigurationValidationPhase,
    in_validator_callback,
    normalize_validators,
    require_validation_timeout,
    validate_candidate,
)

logger = logging.getLogger("ConfigManager")


def _sanitize(value: str) -> str:
    """Neutralize characters in a substituted value that are dangerous in a file
    path or MQTT topic: path separators (``/``, ``\\``), traversal dot sequences
    (``..``), MQTT wildcards (``+``, ``#``), and control characters are each
    replaced with ``_``. Applied only to interpolated values, never to the
    surrounding template, so structural separators in the template are preserved.
    Mirrors the Rust library's ``config::template::sanitize``.
    """
    if value is None:
        return ""
    result = []
    for c in str(value):
        o = ord(c)
        if c in "/\\+#" or o < 0x20 or 0x7F <= o <= 0x9F:
            result.append("_")
        else:
            result.append(c)
    # Collapse traversal sequences (e.g. "..") that remain after separator replacement.
    return "".join(result).replace("..", "_")


# Strict UNS hierarchy level-name rule (future Parquet columns — keep it tight).
_HIERARCHY_LEVEL_NAME = re.compile(r"^[A-Za-z0-9_-]+$")

# The schema default for messaging.requestTimeoutSeconds (seconds).
DEFAULT_REQUEST_TIMEOUT_SECONDS = 30.0


_IN_APPLIED_LISTENER = threading.local()


@dataclass(frozen=True)
class _ConfigSnapshot:
    """One accepted generation; installed with one reference assignment."""

    generation: int
    raw_config: Dict[str, Any]
    component_layer: Dict[str, Any]
    base_layer: Optional[Dict[str, Any]]
    base_source: Optional[str]
    tag_config: TagConfiguration
    heartbeat_config: HeartbeatConfiguration
    health_config: HealthConfiguration
    metric_config: MetricConfiguration
    logging_config: EnhancedLoggingConfiguration
    component_config: Dict[str, Any]
    global_config: Dict[str, Any]
    instances: Dict[str, Dict[str, Any]]
    streaming_config: Optional[Dict[str, Any]]
    credentials_config: Optional[Dict[str, Any]]
    parameters_config: Optional[Dict[str, Any]]
    topic_include_root: bool
    messaging_request_timeout_seconds: float
    component_identity: Optional[MessageIdentity]


def _identity_error(detail: str) -> ValueError:
    """The uniform fail-fast identity-resolution startup error."""
    return ValueError(f"Component identity resolution failed: {detail}")


class ConfigManager:
    def __init__(
        self,
        component_name: str,
        thing_name: str = None,
        validate_config: bool = True,
        platform=None,
        candidate_validators: Optional[
            Mapping[str, ConfigurationCandidateValidator]
        ] = None,
        validation_timeout_secs: float = DEFAULT_CANDIDATE_VALIDATION_TIMEOUT_SECS,
    ):
        if not component_name:
            raise ValueError("Component name cannot be None or empty")

        # The resolved platform (a edgecommons.platform.Platform, or None when constructed outside the
        # resolver/builder). Threaded in BEFORE init() so _apply_config can apply the platform's
        # default logging format (json on KUBERNETES) when the config omits one (FR-RT-3 / FR-LOG-1).
        self._platform = platform
        self._transition_lock = threading.Lock()
        self._snapshot_lock = threading.RLock()
        self._snapshot: Optional[_ConfigSnapshot] = None
        self._candidate_validators = normalize_validators(candidate_validators)
        self._candidate_validation_timeout = require_validation_timeout(
            validation_timeout_secs
        )
        self._last_candidate_validation_errors: Tuple[
            ConfigurationValidationError, ...
        ] = ()
        self._tag_config = None
        self._heartbeat_config = None
        self._health_config = None
        self._metric_config = None
        self._component_config = None
        self._logging_config = None
        self._streaming_config = None
        self._credentials_config = None
        self._parameters_config = None
        self._global_config = {}
        self._instances = {}
        self._change_listeners = []
        self._thing_name = thing_name
        self._component_full_name = component_name
        self._validate_config = validate_config
        self._initializing = True
        self._config_source = "unknown"
        self._config_provider_family = "UNKNOWN"
        self._latest_component_layer: Optional[Dict[str, Any]] = None
        self._latest_base_layer: Optional[Dict[str, Any]] = None
        self._latest_base_source: Optional[str] = None
        # The component's resolved UNS identity (hierarchy + identity values + device +
        # component token, instance "main"), resolved ONCE by init() from the
        # component's OWN config (no shared config) — see get_component_identity().
        # None until init() runs (test/subclass bring-up without init keeps it None,
        # mirroring the Java protected constructor).
        self._component_identity: Optional[MessageIdentity] = None
        # The raw effective config exactly as applied (get_effective_config()); the
        # source for identity resolution and the cfg publisher.
        self._raw_config: Optional[Dict[str, Any]] = None
        # Whether UNS topics carry the first hierarchy value (site) after the ecv1 root
        # — the top-level topic.includeRoot setting (UNS-CANONICAL-DESIGN §2.2 rule 6 /
        # D-U11), default False. Parsed by _apply_config (a hot reload refreshes it).
        # Effective in Uns only with a multi-level hierarchy (D-U25).
        self._topic_include_root = False
        # One-shot flag for the D-U25 includeRoot-with-single-level-hierarchy WARN.
        self._warned_include_root_single_level = False
        # The default request() deadline in seconds — messaging.requestTimeoutSeconds
        # (UNS-CANONICAL-DESIGN §5 / D-U5), default 30; 0 disables. Late-bound onto the
        # messaging client by EdgeCommons right after this manager is constructed.
        self._messaging_request_timeout_seconds = DEFAULT_REQUEST_TIMEOUT_SECONDS

        if "." in component_name:
            self._component_name = component_name.rpartition(".")[-1]
        else:
            self._component_name = component_name

    def init(self):
        try:
            component_layer, base_layer, base_source, config = self._load_effective_configuration()
            with self._transition_lock:
                accepted = self._activate_candidate(
                    component_layer,
                    base_layer,
                    base_source,
                    config,
                    ConfigurationValidationPhase.INITIAL,
                )
            if not accepted:
                raise ConfigurationCandidateRejected(
                    self._last_candidate_validation_errors
                )

            logger.info("Configuration manager initialized successfully")

        except Exception as e:
            logger.error(f"Failed to initialize configuration manager: {e}")
            raise

    def _prepare_snapshot(
        self,
        component_layer: Dict[str, Any],
        base_layer: Optional[Dict[str, Any]],
        base_source: Optional[str],
        config: dict,
        generation: int,
        resolve_identity: bool = True,
    ) -> _ConfigSnapshot:
        """Parse a complete candidate off-side without touching live state."""

        raw_config = copy.deepcopy(config)
        component_config = copy.deepcopy(
            raw_config.get("component", {"global": {}, "instances": []})
        )
        if not isinstance(component_config, dict):
            raise ValueError("component configuration must be an object")
        global_config = copy.deepcopy(component_config.get("global", {}))
        instances = {}
        for instance in component_config.get("instances", []):
            instances[instance["id"]] = copy.deepcopy(instance)

        topic_include_root = self._parse_topic_include_root(raw_config)
        tag_config = TagConfiguration(raw_config.get("tags"))
        logging_config = EnhancedLoggingConfiguration(
            raw_config.get("logging"),
            platform_default_format=profile_logging_format(self._platform),
            correlation=self._logging_correlation(),
        )
        return _ConfigSnapshot(
            generation=generation,
            raw_config=raw_config,
            component_layer=copy.deepcopy(component_layer),
            base_layer=copy.deepcopy(base_layer),
            base_source=base_source,
            tag_config=tag_config,
            heartbeat_config=HeartbeatConfiguration(raw_config.get("heartbeat")),
            health_config=HealthConfiguration(raw_config.get("health")),
            metric_config=MetricConfiguration(raw_config.get("metricEmission")),
            logging_config=logging_config,
            component_config=component_config,
            global_config=global_config,
            instances=instances,
            streaming_config=copy.deepcopy(raw_config.get("streaming")),
            credentials_config=copy.deepcopy(raw_config.get("credentials")),
            parameters_config=copy.deepcopy(raw_config.get("parameters")),
            topic_include_root=topic_include_root,
            messaging_request_timeout_seconds=(
                self._parse_messaging_request_timeout_seconds(raw_config)
            ),
            component_identity=(
                self._resolve_component_identity(raw_config)
                if resolve_identity
                else self._component_identity
            ),
        )

    def _activate_candidate(
        self,
        component_layer: Dict[str, Any],
        base_layer: Optional[Dict[str, Any]],
        base_source: Optional[str],
        config: dict,
        phase: ConfigurationValidationPhase,
    ) -> bool:
        """Schema-check, validate and atomically install one generation."""

        if self._validate_config:
            self._validate_configuration(config)
        current = self._snapshot
        generation = 1 if current is None else current.generation + 1
        prepared = self._prepare_snapshot(
            component_layer,
            base_layer,
            base_source,
            config,
            generation,
            resolve_identity=(phase is ConfigurationValidationPhase.INITIAL),
        )

        redacted_current = None
        if current is not None:
            # Local import avoids a module cycle at import time.
            from edgecommons.config.effective_config_publisher import redact

            redacted_current = redact(current.raw_config)
        errors = validate_candidate(
            self._candidate_validators,
            prepared.raw_config,
            redacted_current,
            phase,
            self._candidate_validation_timeout,
        )
        self._last_candidate_validation_errors = errors
        if errors:
            logger.warning(
                "Configuration candidate rejected in generation %d: %s",
                generation,
                "; ".join(
                    f"{error.validator}:{error.code}: {error.message}"
                    for error in errors
                ),
            )
            return False

        self._install_snapshot(prepared)
        return True

    def _install_snapshot(self, snapshot: _ConfigSnapshot) -> None:
        """Publish a prepared generation with one atomic snapshot swap."""

        with self._snapshot_lock:
            self._snapshot = snapshot
            # Preserve private compatibility fields for older subclasses/tests. Public readers
            # use the snapshot reference and therefore never observe a torn generation.
            self._raw_config = snapshot.raw_config
            self._tag_config = snapshot.tag_config
            self._heartbeat_config = snapshot.heartbeat_config
            self._health_config = snapshot.health_config
            self._metric_config = snapshot.metric_config
            self._logging_config = snapshot.logging_config
            self._component_config = snapshot.component_config
            self._global_config = snapshot.global_config
            self._instances = snapshot.instances
            self._streaming_config = snapshot.streaming_config
            self._credentials_config = snapshot.credentials_config
            self._parameters_config = snapshot.parameters_config
            self._topic_include_root = snapshot.topic_include_root
            self._messaging_request_timeout_seconds = (
                snapshot.messaging_request_timeout_seconds
            )
            self._component_identity = snapshot.component_identity
            self._latest_component_layer = snapshot.component_layer
            self._latest_base_layer = snapshot.base_layer
            self._latest_base_source = snapshot.base_source

        if (
            snapshot.topic_include_root
            and not self._warned_include_root_single_level
            and self._hierarchy_level_count(snapshot.raw_config) == 1
        ):
            self._warned_include_root_single_level = True
            logger.warning(
                "topic.includeRoot=true has no effect with a single-level hierarchy"
                " (hierarchy.levels resolves to one level - the device): the site"
                " position requires a level above the device, so UNS topics stay"
                " rootless. Declare a multi-level hierarchy.levels or remove"
                " topic.includeRoot."
            )

        # Logging changes only after the generation is current. A logging sink failure must not
        # roll back or misreport an otherwise accepted atomic generation.
        try:
            snapshot.logging_config.configure_logging(self)
            logging.Formatter.converter = time.gmtime
        except Exception as exc:  # noqa: BLE001 - accepted config remains accepted
            logger.error("Failed to reconfigure logging for accepted config: %s", exc)

    def _apply_config(self, config: dict):
        """Legacy internal test seam; install without candidate callbacks/listeners.

        Runtime paths use :meth:`_activate_candidate`. This helper retains its historical
        behavior of leaving the already-resolved identity unchanged.
        """

        current = self._snapshot
        generation = 1 if current is None else current.generation + 1
        prepared = self._prepare_snapshot(
            config, None, None, config, generation, resolve_identity=False
        )
        self._install_snapshot(prepared)

    def _gen_instances_map(self):
        """Compatibility helper retained for older subclasses."""

        self._instances = {
            instance["id"]: instance
            for instance in self._component_config.get("instances", [])
        }

    def _logging_correlation(self) -> Dict[str, str]:
        """Best-effort correlation fields for the stdout-JSON sink (FR-LOG-3).

        ``thing`` is the resolved identity; ``pod``/``namespace``/``node`` come from the Kubernetes
        Downward-API env vars (``POD_NAME``/``POD_NAMESPACE``/``NODE_NAME`` — the same vars wired in
        Phase 1b). Absent env vars are simply omitted (no empty/null noise); the JSON formatter also
        drops falsy values. These are only consumed when the JSON sink is active.
        """
        correlation: Dict[str, str] = {}
        if self._thing_name:
            correlation["thing"] = self._thing_name
        for field, env_var in (
            ("pod", ENV_K8S_POD_NAME),
            ("namespace", ENV_K8S_POD_NAMESPACE),
            ("node", ENV_K8S_NODE_NAME),
        ):
            value = os.environ.get(env_var)
            if value:
                correlation[field] = value
        return correlation

    def configuration_changed(self, new_config: dict) -> bool:
        if in_validator_callback():
            logger.warning("Configuration activation requested from a candidate validator; rejected")
            return False
        if getattr(_IN_APPLIED_LISTENER, "active", False):
            logger.warning("Re-entrant configuration activation from an applied listener; rejected")
            return False
        try:
            with self._transition_lock:
                logger.debug("Processing configuration change")
                component_layer, base_layer, base_source, effective_config = (
                    self._effective_from_source_payload(new_config)
                )
                if not self._activate_candidate(
                    component_layer,
                    base_layer,
                    base_source,
                    effective_config,
                    ConfigurationValidationPhase.RELOAD,
                ):
                    return False
                if not self._initializing:
                    self._notify_configuration_changed(effective_config)
            logger.info("Configuration change processed successfully")
            return True
        except Exception as e:
            logger.error(f"Failed to process configuration change: {e}")
            return False

    def reload_from_provider(self) -> bool:
        """Re-fetches the configuration from the active config source and re-applies
        it - the ``reload-config`` command verb's action (DESIGN-uns §9.5).
        Re-invokes :meth:`_load_configuration` (re-reads the file / ConfigMap / env /
        shadow, or re-requests from the config component) and applies the result via
        :meth:`configuration_changed`, which re-validates against the schema and
        notifies the change listeners on success (so a successful reload also
        re-announces the ``cfg`` push, since :class:`~edgecommons.config.effective_config_publisher.EffectiveConfigPublisher`
        is a registered listener) - reject-and-keep on a schema-invalid document.
        Best-effort: any re-fetch failure is logged and reported as ``False`` - a
        reload must never crash a running component.

        :returns: ``True`` when a document was fetched, validated and applied;
            ``False`` when the fetch failed / returned nothing, or the document was
            schema-invalid (the previous configuration is kept)
        """
        if in_validator_callback() or getattr(_IN_APPLIED_LISTENER, "active", False):
            logger.warning("Re-entrant provider reload requested from a configuration callback; rejected")
            return False
        try:
            source_payload = self._load_configuration()
        except Exception as e:
            logger.warning(
                f"reload-config: re-fetch from the '{self._config_source}' source"
                f" failed: {e}"
            )
            return False
        if source_payload is None:
            logger.warning(
                f"reload-config: the '{self._config_source}' source returned no"
                " configuration - keeping the previous configuration"
            )
            return False
        return self.configuration_changed(source_payload)

    @staticmethod
    def _parse_topic_include_root(config: dict) -> bool:
        """Parses the top-level ``topic.includeRoot`` flag (default ``False``). Lenient
        like the other permissive subsystem sections: a missing/non-object ``topic`` or
        a missing/non-boolean ``includeRoot`` yields the default."""
        if not isinstance(config, dict):
            return False
        topic = config.get("topic")
        if not isinstance(topic, dict):
            return False
        include_root = topic.get("includeRoot")
        return include_root is True

    @staticmethod
    def _hierarchy_level_count(config: dict) -> int:
        """Lenient ``hierarchy.levels`` entry count for the D-U25 config WARN: a
        missing/malformed ``hierarchy`` section counts as the zero-config single-level
        default (``["device"]``). Strict validation happens in
        :meth:`_resolve_component_identity` (fail-fast at init); this helper must never
        throw on shapes the WARN check sees first."""
        if not isinstance(config, dict):
            return 1
        hierarchy = config.get("hierarchy")
        if not isinstance(hierarchy, dict):
            return 1
        levels = hierarchy.get("levels")
        if not isinstance(levels, list) or not levels:
            return 1
        return len(levels)

    @staticmethod
    def _parse_messaging_request_timeout_seconds(config: dict) -> float:
        """Parses ``messaging.requestTimeoutSeconds`` (§5 / D-U5): a non-negative
        number of seconds (fractions allowed), default 30. Lenient — a missing/
        non-object ``messaging`` section, a missing/non-number value, or a negative
        value (which the schema rejects at startup anyway) all yield the default.
        ``0`` is a valid explicit value meaning "disabled"."""
        if not isinstance(config, dict):
            return DEFAULT_REQUEST_TIMEOUT_SECONDS
        messaging = config.get("messaging")
        if not isinstance(messaging, dict):
            return DEFAULT_REQUEST_TIMEOUT_SECONDS
        value = messaging.get("requestTimeoutSeconds")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            return DEFAULT_REQUEST_TIMEOUT_SECONDS
        return DEFAULT_REQUEST_TIMEOUT_SECONDS if value < 0 else float(value)

    def is_topic_include_root(self) -> bool:
        """Whether UNS topics built by ``gg.uns()`` / ``gg.instance(id).uns()`` carry
        the first hierarchy value (``site``) between the ``ecv1`` root and the device —
        the top-level ``topic.includeRoot`` setting, default ``False``. Note that
        ``Uns`` applies it only when the hierarchy is multi-level (D-U25)."""
        snapshot = self._snapshot
        return False if snapshot is None else snapshot.topic_include_root

    def get_messaging_request_timeout(self) -> float:
        """The default ``request()`` deadline resolved from
        ``messaging.requestTimeoutSeconds`` (UNS-CANONICAL-DESIGN §5 / D-U5), in
        seconds; ``0`` when disabled. ``EdgeCommons`` late-binds this onto the messaging
        client right after this manager is constructed; an explicit per-call timeout on
        ``request()`` always wins over this default."""
        snapshot = self._snapshot
        value = (
            DEFAULT_REQUEST_TIMEOUT_SECONDS
            if snapshot is None
            else snapshot.messaging_request_timeout_seconds
        )
        return 0.0 if value <= 0 else value

    def get_component_identity(self) -> Optional[MessageIdentity]:
        """The component's resolved UNS identity (instance ``"main"``), resolved once
        by ``init()`` from the component's OWN config:

        1. ``levels`` = top-level ``hierarchy.levels`` when present, else the
           zero-config default ``["device"]``.
        2. Level names must match ``^[A-Za-z0-9_-]+$``, be unique and non-empty.
        3. Every level except the last takes its value from the top-level ``identity``
           config object (a missing value is a startup error naming the level); the
           LAST level's value is the resolved thing name (the existing identity chain).
        4. An ``identity`` key equal to the last level name, or not among the declared
           non-device levels, is a startup error (typo protection the schema cannot
           express).
        5. Every value and the component short name pass through the template
           sanitizer.

        :returns: the resolved identity, or ``None`` when this manager was constructed
            without ``init()`` (test/subclass bring-up — no config was resolved)
        """
        snapshot = self._snapshot
        return None if snapshot is None else copy.deepcopy(snapshot.component_identity)

    def _resolve_component_identity(self, config: dict) -> MessageIdentity:
        """Resolves the component identity from the applied config (see
        :meth:`get_component_identity` for the algorithm). Called once from ``init()``;
        fail-fast with a precise ``ValueError``."""
        # 1. levels = hierarchy.levels if present, else the zero-config default.
        levels = []
        if isinstance(config, dict) and "hierarchy" in config:
            hierarchy = config.get("hierarchy")
            if not isinstance(hierarchy, dict) or "levels" not in hierarchy:
                raise _identity_error("'hierarchy' must be an object with a 'levels' array")
            raw_levels = hierarchy.get("levels")
            if not isinstance(raw_levels, list) or not raw_levels:
                raise _identity_error(
                    "'hierarchy.levels' must be a non-empty array of level names"
                )
            for level in raw_levels:
                if not isinstance(level, str):
                    raise _identity_error("'hierarchy.levels' entries must be strings")
                levels.append(level)
        else:
            levels.append("device")

        # 2. Level names: strict charset, unique, non-empty.
        seen = set()
        for level in levels:
            if not level or not _HIERARCHY_LEVEL_NAME.match(level):
                raise _identity_error(
                    f"invalid hierarchy level name '{level}' (must match ^[A-Za-z0-9_-]+$)"
                )
            if level in seen:
                raise _identity_error(f"duplicate hierarchy level name '{level}'")
            seen.add(level)
        device_level = levels[-1]
        value_levels = levels[:-1]

        # 3/4. The `identity` config object supplies every level's value except the
        #      last; keys must be exactly (a subset of) the non-device levels.
        identity_config = {}
        if isinstance(config, dict) and "identity" in config:
            identity_raw = config.get("identity")
            if not isinstance(identity_raw, dict):
                raise _identity_error("'identity' must be an object of level-name -> value")
            identity_config = identity_raw
        for key in identity_config:
            if key == device_level:
                raise _identity_error(
                    f"'identity.{key}' must not be set: '{device_level}' is the last"
                    " hierarchy level (the device) and its value is always the resolved"
                    " thing name"
                )
            if key not in value_levels:
                raise _identity_error(
                    f"'identity.{key}' is not a declared hierarchy level; expected"
                    f" keys: {value_levels}"
                )

        hier = []
        missing = []
        for level in value_levels:
            value = identity_config.get(level)
            if not isinstance(value, str) or value == "":
                missing.append(level)
                continue
            hier.append(HierEntry(level, self._sanitized_identity_value(level, value)))
        if missing:
            raise _identity_error(
                f"the top-level 'identity' config object is missing value(s) for"
                f" hierarchy level(s) {missing} (hierarchy.levels = {levels}; the last"
                f" level '{device_level}' is the resolved thing name and must not be"
                " configured)"
            )

        # The device (last level) value is the resolved thing name (PlatformResolver
        # chain).
        if not self._thing_name:
            raise _identity_error(
                f"the device level '{device_level}' value (the resolved thing name) is"
                " not available"
            )
        hier.append(HierEntry(device_level,
                              self._sanitized_identity_value(device_level, self._thing_name)))

        # 5. component = explicit token when configured, else sanitized short name.
        if not self._component_name:
            raise _identity_error("the component name is not available")
        configured_token = self._configured_component_token(config)
        component_token = self._sanitized_identity_value(
            "component", configured_token if configured_token is not None else self._component_name
        )
        return MessageIdentity(hier, component_token, MessageIdentity.DEFAULT_INSTANCE)

    @staticmethod
    def _configured_component_token(config: dict) -> Optional[str]:
        component = config.get("component")
        if component is None:
            return None
        if not isinstance(component, dict):
            raise _identity_error("'component' must be an object when configuring 'component.token'")
        if "token" not in component:
            return None
        token = component.get("token")
        if not isinstance(token, str) or token == "":
            raise _identity_error("'component.token' must be a non-empty string")
        return token

    @staticmethod
    def _sanitized_identity_value(what: str, raw_value: str) -> str:
        """Sanitizes an identity value via the template sanitizer, WARN-logging when it
        changed."""
        sanitized = _sanitize(raw_value)
        if sanitized != raw_value:
            logger.warning(
                f"Identity value for '{what}' contained reserved characters and was"
                f" sanitized: '{raw_value}' -> '{sanitized}'"
            )
        return sanitized

    @staticmethod
    def sanitize(value: str) -> str:
        """The template-value sanitizer (``/ \\ + #``, control chars incl. C1 -> ``_``;
        remaining ``..`` -> ``_``). Public because it is also the normative UNS
        channel-token sanitizer (UNS-CANONICAL-DESIGN §2.2 rule 1 / D-U26): the
        ``uns()`` token rule is exactly this blacklist, so "sanitized => publishable"
        holds. The metric ``messaging`` target uses it to turn a metric name into the
        ``metric/{metricName}`` channel token (§4.3)."""
        return _sanitize(value)

    def get_effective_config(self) -> Optional[Dict[str, Any]]:
        """The raw effective configuration exactly as last applied (init or hot
        reload), or ``None`` before any config was applied. The source the
        effective-config (``cfg``) publisher redacts + announces (UNS-CANONICAL-DESIGN
        §4.3). Distinct from :meth:`get_full_config`, which reconstructs a normalized
        view from the typed config models."""
        snapshot = self._snapshot
        return None if snapshot is None else copy.deepcopy(snapshot.raw_config)

    def resolve_template(self, template: str) -> str:
        snapshot = self._snapshot
        ret_val = template
        if "{ThingName}" in template:
            ret_val = ret_val.replace("{ThingName}", _sanitize(self._thing_name))
        if "{ComponentName}" in template:
            ret_val = ret_val.replace("{ComponentName}", _sanitize(self._component_name))
        if "{ComponentFullName}" in template:
            ret_val = ret_val.replace("{ComponentFullName}", _sanitize(self._component_full_name))
        component_identity = (
            self._component_identity
            if snapshot is None
            else snapshot.component_identity
        )
        if component_identity is not None:
            for entry in component_identity.hier:
                identity_template = "{" + entry.level + "}"
                if identity_template in ret_val:
                    ret_val = ret_val.replace(identity_template, _sanitize(entry.value))
        tag_config = self._tag_config if snapshot is None else snapshot.tag_config
        tag_dict = {} if tag_config is None else tag_config.to_dict()
        for k in tag_dict.keys():
            key_template = "{" + k + "}"
            if key_template in ret_val:
                ret_val = ret_val.replace(key_template, _sanitize(tag_dict[k]))
        return ret_val

    def _load_configuration(self) -> dict:
        """Default implementation returns empty config. Subclasses should override."""
        return {"component": {}}

    def _load_effective_configuration(self):
        source_payload = self._load_configuration()
        if source_payload is None:
            source_payload = {"component": {}}
        return self._effective_from_source_payload(
            source_payload,
        )

    def _effective_from_source_payload(
        self,
        source_payload,
    ):
        if self._config_provider_family == "CONFIG_COMPONENT":
            parsed = parse_config_component_payload(
                source_payload,
                request_component=self._expected_config_component_token(),
            )
            component_layer = parsed.configs[-1]
            base_layer = None
            base_source = f"config-component:lineage:{parsed.catalog_version}"
            effective = deep_merge_layers(parsed.configs, self._warn_type_conflict)
            logger.info(
                "CONFIG_COMPONENT lineage applied: catalogVersion=%s component=%s layers=%d",
                parsed.catalog_version,
                parsed.component,
                len(parsed.layers),
            )
        else:
            if not isinstance(source_payload, dict):
                raise HierarchicalConfigError("CONFIG_LAYER_INVALID", "config document must be a JSON object")
            component_layer = source_payload
            base_layer = None
            base_source = None
            effective = component_layer
            logger.info("Direct config source supplied one effective document")
        return component_layer, base_layer, base_source, effective

    def _expected_config_component_token(self) -> str:
        return self.sanitize(self.get_component_name())

    def _commit_layers(
        self,
        component_layer: Dict[str, Any],
        base_layer: Optional[Dict[str, Any]],
        base_source: Optional[str],
    ) -> None:
        self._latest_component_layer = component_layer
        self._latest_base_layer = base_layer
        self._latest_base_source = base_source

    @staticmethod
    def _warn_type_conflict(path: str, left_type: str, right_type: str) -> None:
        logger.warning(
            "Hierarchical config type conflict at %s: %s replaced by %s from later layer",
            path,
            left_type,
            right_type,
        )

    def get_global_config(self) -> dict:
        snapshot = self._snapshot
        return {} if snapshot is None else copy.deepcopy(snapshot.global_config)

    def get_instance_ids(self) -> list:
        snapshot = self._snapshot
        return [] if snapshot is None else list(snapshot.instances)

    def get_instance_config(self, inst_id) -> dict:
        snapshot = self._snapshot
        if snapshot is None:
            raise KeyError(inst_id)
        return copy.deepcopy(snapshot.instances[inst_id])

    def get_tag_config(self) -> TagConfiguration:
        snapshot = self._snapshot
        return self._tag_config if snapshot is None else snapshot.tag_config

    def get_heartbeat_config(self) -> HeartbeatConfiguration:
        snapshot = self._snapshot
        return self._heartbeat_config if snapshot is None else snapshot.heartbeat_config

    def get_health_config(self) -> HealthConfiguration:
        """The parsed ``health`` config section (Phase 1c). Never ``None`` after init — defaults to a
        schema-aligned :class:`HealthConfiguration` with ``enabled=None`` when the section is absent."""
        snapshot = self._snapshot
        return self._health_config if snapshot is None else snapshot.health_config

    def get_metric_config(self) -> MetricConfiguration:
        snapshot = self._snapshot
        return self._metric_config if snapshot is None else snapshot.metric_config

    def get_platform(self):
        """The resolved :class:`~edgecommons.platform.platform.Platform` (or ``None`` when constructed
        outside the resolver/builder).

        Threaded in by the builder so subsystem initializers can apply platform-profile defaults
        without a new resolver dependency. Used as the middle precedence tier by the logging
        configurator (default logging format) and by
        :class:`~edgecommons.metrics.metric_emitter.MetricEmitter` (default metric target — prometheus
        on KUBERNETES, FR-MET-4 / FR-RT-3).
        """
        return self._platform

    def get_logging_config(self) -> EnhancedLoggingConfiguration:
        snapshot = self._snapshot
        return self._logging_config if snapshot is None else snapshot.logging_config

    def get_thing_name(self) -> str:
        return self._thing_name

    def get_component_name(self) -> str:
        return self._component_name

    def get_component_full_name(self) -> str:
        return self._component_full_name

    def add_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        if listener is None:
            raise ValueError("Listener cannot be None")
        self._change_listeners.append(listener)
        logger.debug(f"Added configuration change listener: {listener}")

    def get_config_source(self) -> str:
        return self._config_source
        
    def _validate_configuration(self, config: Dict[str, Any]) -> None:
        """Validates configuration; raises on a schema-invalid config so the caller rejects it.

        A schema-invalid config raises in **both** the init and the hot-reload paths. ``__init__``
        aborts startup; a hot reload is rejected by :meth:`configuration_changed`, which keeps the
        last-good config and does **not** notify listeners (reject-and-keep) — at parity with the
        Java/Rust/TS libraries. A missing validator dependency (``ImportError``) is a soft skip, not
        a failure. Override in subclasses for specific validation.
        """
        try:
            from edgecommons.validation.configuration_validator import (
                ConfigurationValidator,
            )
            ConfigurationValidator.validate(config)
            logger.debug("Configuration validation passed")

        except ImportError:
            logger.debug("Configuration validator not available, skipping validation")
        except Exception as e:
            logger.error(f"Configuration validation failed; rejecting configuration: {e}")
            raise
                
    def complete_initialization(self) -> None:
        """Marks initialization as complete."""
        self._initializing = False
        logger.debug("Configuration manager initialization completed")

    def close(self) -> None:
        """Release any resources held by this config manager (e.g. file watchers).

        No-op by default; subclasses such as FileConfigManager override this to
        stop their background threads.
        """
        pass
        
    def _notify_configuration_changed(self, new_config: Dict[str, Any]) -> None:
        """Notifies all registered listeners of configuration changes."""
        logger.info(f"Notifying {len(self._change_listeners)} configuration change listeners")
        
        for listener in self._change_listeners:
            try:
                if listener is not None:
                    _IN_APPLIED_LISTENER.active = True
                    result = listener.on_configuration_change(copy.deepcopy(new_config))
                    if not result:
                        logger.warning(f"Listener {listener} returned False for configuration change")
                else:
                    logger.error("Configuration change listener is None")
            except Exception as e:
                logger.error(f"Error notifying configuration change listener {listener}: {e}")
            finally:
                _IN_APPLIED_LISTENER.active = False
                
    def remove_config_change_listener(self, listener: ConfigurationChangeListener) -> None:
        """Removes a configuration change listener."""
        if listener is None:
            raise ValueError("Listener cannot be None")
            
        if listener in self._change_listeners:
            self._change_listeners.remove(listener)
            logger.debug(f"Removed configuration change listener: {listener}")
        else:
            logger.warning(f"Attempted to remove non-existent listener: {listener}")
            
    def get_full_config(self) -> Dict[str, Any]:
        """Returns the accepted effective configuration snapshot."""
        snapshot = self._snapshot
        return copy.deepcopy(snapshot.raw_config) if snapshot is not None else {}

    def get_generation(self) -> int:
        """The accepted configuration generation (zero before the initial commit)."""

        snapshot = self._snapshot
        return 0 if snapshot is None else snapshot.generation

    def get_last_candidate_validation_errors(
        self,
    ) -> Tuple[ConfigurationValidationError, ...]:
        """Stable errors from the most recent rejected candidate, else an empty tuple."""

        return tuple(self._last_candidate_validation_errors)
        
    def is_validation_enabled(self) -> bool:
        """Returns whether configuration validation is enabled."""
        return self._validate_config
        
    def is_initializing(self) -> bool:
        """Returns whether the configuration manager is still initializing."""
        return self._initializing
