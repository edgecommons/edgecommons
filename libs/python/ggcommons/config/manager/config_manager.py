import logging
import os
import re
import time
from typing import Dict, Any, Optional

from ggcommons.messaging.identity import HierEntry, MessageIdentity
from ggcommons.platform.resolver import (
    ENV_K8S_NODE_NAME,
    ENV_K8S_POD_NAME,
    ENV_K8S_POD_NAMESPACE,
    profile_logging_format,
)
from ggcommons.config.heartbeat_config import HeartbeatConfiguration
from ggcommons.config.health_config import HealthConfiguration
from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.config.tag_config import TagConfiguration
from ggcommons.config.enhanced_logging_config import EnhancedLoggingConfiguration
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener

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
    ):
        if not component_name:
            raise ValueError("Component name cannot be None or empty")

        # The resolved platform (a ggcommons.platform.Platform, or None when constructed outside the
        # resolver/builder). Threaded in BEFORE init() so _apply_config can apply the platform's
        # default logging format (json on KUBERNETES) when the config omits one (FR-RT-3 / FR-LOG-1).
        self._platform = platform
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
        # messaging client by GGCommons right after this manager is constructed.
        self._messaging_request_timeout_seconds = DEFAULT_REQUEST_TIMEOUT_SECONDS

        if "." in component_name:
            self._component_name = component_name.rpartition(".")[-1]
        else:
            self._component_name = component_name

    def init(self):
        try:
            config = self._load_configuration()
            if config is None:
                config = {"component": {}}

            # Validate configuration if enabled
            if self._validate_config:
                self._validate_configuration(config)

            self._apply_config(config)

            # Resolve the component's UNS identity ONCE, from this component's own
            # config (top-level `hierarchy` + `identity`), fail-fast on any
            # inconsistency (UNS-CANONICAL-DESIGN §1.5).
            self._component_identity = self._resolve_component_identity(config)

            logger.info("Configuration manager initialized successfully")

        except Exception as e:
            logger.error(f"Failed to initialize configuration manager: {e}")
            raise

    def _apply_config(self, config: dict):
        # Retain the raw effective config verbatim: the source for identity resolution
        # and the effective-config (cfg) publisher (UNS-CANONICAL-DESIGN §4.3).
        self._raw_config = config

        # UNS topic options + the request() deadline default (re-parsed on hot reload).
        self._topic_include_root = self._parse_topic_include_root(config)
        # D-U25: includeRoot needs a level ABOVE the device to prepend — with a
        # single-level hierarchy (the zero-config ["device"] default) hier[0] IS the
        # device, so the setting is a no-op in Uns (prepending would duplicate the
        # device). Tell the user once.
        if (
            self._topic_include_root
            and not self._warned_include_root_single_level
            and self._hierarchy_level_count(config) == 1
        ):
            self._warned_include_root_single_level = True
            logger.warning(
                "topic.includeRoot=true has no effect with a single-level hierarchy"
                " (hierarchy.levels resolves to one level - the device): the site"
                " position requires a level above the device, so UNS topics stay"
                " rootless (D-U25). Declare a multi-level hierarchy.levels or remove"
                " topic.includeRoot."
            )
        self._messaging_request_timeout_seconds = (
            self._parse_messaging_request_timeout_seconds(config)
        )

        # Tags first: the log file path template ({ThingName}/{ComponentName}/{tag})
        # is resolved during logging setup below, so tag_config must already exist.
        tag_json = config.get("tags")
        self._tag_config = TagConfiguration(tag_json)

        logging_json = config.get("logging")
        # FR-RT-3 / FR-LOG-1: the platform-profile default logging format (json on KUBERNETES) is
        # the middle precedence tier — applied only when the config omits `python_format`.
        platform_default_format = profile_logging_format(self._platform)
        self._logging_config = EnhancedLoggingConfiguration(
            logging_json,
            platform_default_format=platform_default_format,
            correlation=self._logging_correlation(),
        )
        # configure_logging wires the console handler (text or, under the `json` token, the
        # stdout-JSON layout). Off the JSON sink and when logging.fileLogging.enabled it also wires a
        # size-rotated RotatingFileHandler (maxFileSize / backupCount); under the JSON sink in-process
        # rotation is intentionally skipped (FR-LOG-2 — the cluster log agent owns rotation). It
        # clears existing handlers first, so a config hot-reload reconfigures cleanly without leaking.
        self._logging_config.configure_logging(self)
        logging.Formatter.converter = time.gmtime

        heartbeat_json = config.get("heartbeat")
        self._heartbeat_config = HeartbeatConfiguration(heartbeat_json)

        # Health server config (Phase 1c health slice). Always constructed (schema-aligned defaults)
        # so GGCommons._init_health can read port/paths even when the section is absent; `enabled`
        # stays None to let the platform-profile default decide (on for KUBERNETES, FR-RT-3).
        health_json = config.get("health")
        self._health_config = HealthConfiguration(health_json)

        metric_json = config.get("metricEmission")
        self._metric_config = MetricConfiguration(metric_json)

        self._component_config = config.get("component", {"global": {}, "instances": []})
        self._global_config = self._component_config.get("global", {})
        self._gen_instances_map()

        # Retain the raw `streaming` section verbatim (no typed parsing in Python — the native
        # ggstreamlog core owns the streaming schema). Kept so get_full_config() exposes it to
        # GGCommons._init_streaming(); without this the section is dropped and streaming never opens.
        self._streaming_config = config.get("streaming")
        # Likewise retain the raw `credentials` section so GGCommons._init_credentials() can find it.
        self._credentials_config = config.get("credentials")
        # And the raw `parameters` section so GGCommons._init_parameters() can find it.
        self._parameters_config = config.get("parameters")

    def _gen_instances_map(self):
        # Rebuild from scratch so a hot reload that removes an instance does not
        # leave a stale entry behind.
        self._instances = {}
        if "instances" in self._component_config:
            for instance in self._component_config["instances"]:
                self._instances[instance["id"]] = instance
                logger.debug(f"loaded config for {self._instances[instance['id']]}")

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
        try:
            logger.debug("Processing configuration change")
            
            # Validate new configuration if enabled
            if self._validate_config:
                self._validate_configuration(new_config)
                
            # Apply the new configuration
            self._apply_config(new_config)
            
            # Notify listeners only if not initializing
            if not self._initializing:
                self._notify_configuration_changed(new_config)
                
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
        re-announces the ``cfg`` push, since :class:`~ggcommons.config.effective_config_publisher.EffectiveConfigPublisher`
        is a registered listener) - reject-and-keep on a schema-invalid document.
        Best-effort: any re-fetch failure is logged and reported as ``False`` - a
        reload must never crash a running component.

        :returns: ``True`` when a document was fetched, validated and applied;
            ``False`` when the fetch failed / returned nothing, or the document was
            schema-invalid (the previous configuration is kept)
        """
        try:
            new_config = self._load_configuration()
        except Exception as e:
            logger.warning(
                f"reload-config: re-fetch from the '{self._config_source}' source"
                f" failed: {e}"
            )
            return False
        if new_config is None:
            logger.warning(
                f"reload-config: the '{self._config_source}' source returned no"
                " configuration - keeping the previous configuration"
            )
            return False
        return self.configuration_changed(new_config)

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
        return self._topic_include_root

    def get_messaging_request_timeout(self) -> float:
        """The default ``request()`` deadline resolved from
        ``messaging.requestTimeoutSeconds`` (UNS-CANONICAL-DESIGN §5 / D-U5), in
        seconds; ``0`` when disabled. ``GGCommons`` late-binds this onto the messaging
        client right after this manager is constructed; an explicit per-call timeout on
        ``request()`` always wins over this default."""
        return (
            0.0
            if self._messaging_request_timeout_seconds <= 0
            else self._messaging_request_timeout_seconds
        )

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
        return self._component_identity

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

        # 5. component = sanitized short name.
        if not self._component_name:
            raise _identity_error("the component short name is not available")
        component_token = self._sanitized_identity_value("component", self._component_name)
        return MessageIdentity(hier, component_token, MessageIdentity.DEFAULT_INSTANCE)

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
        return self._raw_config

    def resolve_template(self, template: str) -> str:
        ret_val = template
        if "{ThingName}" in template:
            ret_val = ret_val.replace("{ThingName}", _sanitize(self._thing_name))
        if "{ComponentName}" in template:
            ret_val = ret_val.replace("{ComponentName}", _sanitize(self._component_name))
        if "{ComponentFullName}" in template:
            ret_val = ret_val.replace("{ComponentFullName}", _sanitize(self._component_full_name))
        tag_dict = {} if self._tag_config is None else self._tag_config.to_dict()
        for k in tag_dict.keys():
            key_template = "{" + k + "}"
            if key_template in template:
                ret_val = ret_val.replace(key_template, _sanitize(tag_dict[k]))
        return ret_val

    def _load_configuration(self) -> dict:
        """Default implementation returns empty config. Subclasses should override."""
        return {"component": {}}

    def get_global_config(self) -> dict:
        return self._global_config

    def get_instance_ids(self) -> list:
        return [*self._instances]

    def get_instance_config(self, inst_id) -> dict:
        return self._instances[inst_id]

    def get_tag_config(self) -> TagConfiguration:
        return self._tag_config

    def get_heartbeat_config(self) -> HeartbeatConfiguration:
        return self._heartbeat_config

    def get_health_config(self) -> HealthConfiguration:
        """The parsed ``health`` config section (Phase 1c). Never ``None`` after init — defaults to a
        schema-aligned :class:`HealthConfiguration` with ``enabled=None`` when the section is absent."""
        return self._health_config

    def get_metric_config(self) -> MetricConfiguration:
        return self._metric_config

    def get_platform(self):
        """The resolved :class:`~ggcommons.platform.platform.Platform` (or ``None`` when constructed
        outside the resolver/builder).

        Threaded in by the builder so subsystem initializers can apply platform-profile defaults
        without a new resolver dependency. Used as the middle precedence tier by the logging
        configurator (default logging format) and by
        :class:`~ggcommons.metrics.metric_emitter.MetricEmitter` (default metric target — prometheus
        on KUBERNETES, FR-MET-4 / FR-RT-3).
        """
        return self._platform

    def get_logging_config(self) -> EnhancedLoggingConfiguration:
        return self._logging_config

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
            from ggcommons.validation.configuration_validator import (
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
                    result = listener.on_configuration_change(new_config)
                    if not result:
                        logger.warning(f"Listener {listener} returned False for configuration change")
                else:
                    logger.error("Configuration change listener is None")
                    
            except Exception as e:
                logger.error(f"Error notifying configuration change listener {listener}: {e}")
                
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
        """Returns the complete configuration object."""
        full = {
            'component': self._component_config,
            'tags': self._tag_config.to_dict() if self._tag_config else {},
            'heartbeat': self._heartbeat_config.to_dict() if self._heartbeat_config else {},
            'metricEmission': self._metric_config.to_dict() if self._metric_config else {},
            'logging': self._logging_config.to_dict() if self._logging_config else {}
        }
        # Surface the raw streaming section (if any) so GGCommons._init_streaming() can find it.
        if self._streaming_config is not None:
            full['streaming'] = self._streaming_config
        if self._credentials_config is not None:
            full['credentials'] = self._credentials_config
        if self._parameters_config is not None:
            full['parameters'] = self._parameters_config
        return full
        
    def is_validation_enabled(self) -> bool:
        """Returns whether configuration validation is enabled."""
        return self._validate_config
        
    def is_initializing(self) -> bool:
        """Returns whether the configuration manager is still initializing."""
        return self._initializing
