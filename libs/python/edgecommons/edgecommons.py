"""
EdgeCommons main class with builder support.

This module provides the main EdgeCommons class. Component code accesses the
underlying subsystems through typed accessors (get_config_manager / get_messaging
/ get_metrics) rather than a service registry — matching the Java and Rust
libraries, which depend on the concrete ConfigManager / MessagingClient /
MetricEmitter directly.
"""

import argparse
import logging
import os
import signal
import sys
import threading
from datetime import datetime, timezone
from enum import Enum
from typing import Callable, Dict, List, Optional
from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.config_manager_builder import ConfigManagerBuilder
from edgecommons.platform import (
    Platform,
    Transport,
    ResolverInputs,
    resolve_profile,
)

logger = logging.getLogger(__name__)


class ConfigSource(str, Enum):
    """Configuration source passed via -c/--config."""
    FILE = "FILE"
    ENV = "ENV"
    GG_CONFIG = "GG_CONFIG"
    CONFIGMAP = "CONFIGMAP"
    SHADOW = "SHADOW"
    CONFIG_COMPONENT = "CONFIG_COMPONENT"


class EdgeCommons:
    """
    Main entry point for the EdgeCommons framework with enhanced features.
    
    Provides dependency injection, service registry, and improved initialization.
    """

    def __init__(self, component_name: str, args: List[str], 
                 app_options: Optional[argparse.ArgumentParser] = None,
                 receive_own_messages: bool = True,
                 initial_ready: bool = True,
                 configuration_validators: Optional[Dict[str, Callable]] = None,
                 configuration_validation_timeout: float = 5.0,
                 command_configurers: Optional[List[Callable]] = None):
        """
        Initialize EdgeCommons with enhanced features.
        
        Args:
            component_name: The fully qualified component name
            args: Command line arguments
            app_options: Optional custom argument parser
            receive_own_messages: Whether to receive own messages (IPC only)
            
        Raises:
            ValueError: If component_name is None or empty
        """
        if not component_name:
            raise ValueError("Component name cannot be None or empty")
            
        self._component_name = component_name
        # Set before parsing, connecting, loading configuration, or starting any
        # externally observable endpoint: initial_ready(False) can never flicker true.
        self._initial_ready = bool(initial_ready)
        self._configuration_validators = dict(configuration_validators or {})
        self._configuration_validation_timeout = configuration_validation_timeout
        self._command_configurers = list(command_configurers or [])
        self._config_manager: Optional[ConfigManager] = None
        self._heartbeat = None
        # The library-owned cfg publisher (UNS-CANONICAL-DESIGN §4.3): announces the
        # effective (redacted) configuration on ecv1/{device}/{component}/main/cfg at
        # startup and on every configuration change.
        self._effective_config_publisher = None
        # The library-owned _bcast republish listener (DESIGN-uns §9.3/§9.4, the
        # late-join lever): subscribes ecv1/{device}/_bcast/cmd/republish-state|
        # republish-cfg (component scope, no instance - D-U28) on the primary connection
        # and re-announces state/cfg out of
        # band (jittered, coalesced) when the uns-bridge - or a console - broadcasts a
        # republish command.
        self._republish_listener = None
        # The library-owned command inbox (DESIGN-uns §7.3/§9.5, the minimal
        # commands() facade - edge-console slice S2): subscribes both the
        # component-scope inbox wildcard ecv1/{device}/{component}/cmd/# and the
        # instance-scope ecv1/{device}/{component}/+/cmd/# (D-U28) on the primary
        # connection and dispatches cmd envelopes by verb - built-ins ping /
        # reload-config / get-configuration answer the console out of the box; apps add
        # custom verbs via get_commands().register().
        self._command_inbox = None
        # The component-identity-bound UNS topic builder (component scope, no instance
        # token - D-U28), lazily bound on first uns() from the resolved component
        # identity + topic.includeRoot (UNS-CANONICAL-DESIGN §2).
        self._uns = None
        # Cached per-id instance handles (UNS-CANONICAL-DESIGN §3, D-U3): instance(id)
        # returns the same EdgeCommonsInstance for the same id.
        self._instance_handles = {}
        # D-U28: the component-scope handle (no instance token) backing
        # data()/events()/app(); lazily built and cached, mirroring uns().
        self._component_handle = None
        # The clock the data()/events() publish facades use for their time defaults
        # (serverTs/timestamp -> now), injected so tests can pin a fixed clock
        # (DESIGN-class-facades §2, mirrors Java's Clock.systemUTC()).
        self._clock = lambda: datetime.now(timezone.utc)
        self._streams = None
        self._stream_metrics = None
        self._credentials = None
        self._credential_metrics = None
        self._parameters = None
        # Library-owned UNS log publisher. The root logging handler is installed before
        # config loads and stays disabled until logging.publish enables capture.
        from edgecommons.logs import LogService

        self._logs = LogService()
        self._logs.install_handler()
        # Phase 1c health slice: readiness state + HTTP health server + SIGTERM bookkeeping.
        # Created defensively before the try so the shutdown path (also reached on a failed init)
        # can null-guard them.
        self._readiness = None
        self._health_server = None
        self._sigterm_installed = False
        self._prev_sigterm_handler = None
        self._sigint_installed = False
        self._prev_sigint_handler = None

        try:
            # Process command line arguments
            parsed_args = self._process_args(component_name, args, app_options)

            # Initialize messaging FIRST: the GG_CONFIG / SHADOW / CONFIG_COMPONENT
            # config sources load the component configuration over messaging, so the
            # MessagingClient must be available before the config manager is built.
            self._init_messaging(parsed_args, receive_own_messages)

            # Initialize configuration manager
            self._init_config_manager(component_name, parsed_args)

            # UNS-CANONICAL-DESIGN §5 / D-U5 (§1.5 init order): late-bind the request()
            # default deadline from messaging.requestTimeoutSeconds now that the
            # ConfigManager exists. Messaging is initialized BEFORE config loads (the
            # IPC-backed config sources need it), so until this bind the provider's
            # built-in 30 s applied — deliberately, giving the CONFIG_COMPONENT
            # bootstrap request a deadline instead of hanging forever.
            #
            # §4.1 / D-U24: late-bind the reserved-class guard's topic.includeRoot flag
            # the same way (default False pre-bind - nothing publishes rooted topics
            # pre-config). D-U27: bind the EFFECTIVE root (includeRoot AND a
            # multi-level hierarchy) so the guard's position-5 check agrees with
            # topic-building, which no-ops includeRoot on a single-level hierarchy
            # (D-U25); otherwise a warned single-level+includeRoot misconfig would
            # false-positive on a legit app/evt/data channel whose first token is a
            # reserved word.
            from edgecommons.messaging.messaging_client import MessagingClient as _MC
            _MC.set_default_request_timeout(
                self._config_manager.get_messaging_request_timeout()
            )
            _identity = self._config_manager.get_component_identity()
            _MC.set_guard_include_root(
                self._config_manager.is_topic_include_root()
                and _identity is not None
                and len(_identity.hier) >= 2
            )
            self._init_logs(_MC)

            # Logging is configured inside _init_config_manager (via the config manager's
            # _apply_config -> configure_logging). Defer the startup-fact lines to here so they
            # land AFTER logging is configured rather than being dropped during early bootstrap.
            from edgecommons.messaging.messaging_client import MessagingClient
            logger.info(
                "platform resolved: platform=%s transport=%s configSource=%s identity=%s",
                parsed_args.platform.value,
                parsed_args.transport.value,
                parsed_args.config[0],
                parsed_args.identity,
            )
            if MessagingClient.connected():
                logger.info("messaging connected (transport=%s)", parsed_args.transport.value)

            # Initialize metric emitter
            self._init_metrics()
            
            # Initialize heartbeat
            self._init_heartbeat()

            # Credentials / local vault (only when a `credentials` config section is present).
            # Opened BEFORE streaming so the streaming config can reference vault secrets via
            # {"$secret": ...}, mirroring the Rust build() order.
            self._init_credentials()

            # Parameters (only when a `parameters` config section is present) — externalized config
            # parameters from SSM / a mounted dir / env, offline-first cache. Sibling of credentials.
            self._init_parameters()

            # Telemetry streaming (only when a `streaming` config section is present, so components
            # that don't use it never load the native library). Resolves $secret refs first.
            self._init_streaming()

            # HTTP health server + readiness state (Phase 1c health slice). Always builds the
            # readiness state (so set_ready works); starts the server only when enabled (explicit
            # health.enabled ▸ on-by-default on KUBERNETES ▸ off).
            self._init_health(parsed_args)

            # Wire SIGTERM -> graceful shutdown (FR-HB-2). The LIBRARY owns this now, so apps no
            # longer need their own handler. Installed unconditionally (also matters on GREENGRASS,
            # which sends SIGTERM on stop); guarded to the main thread.
            self._install_signal_handlers()

            # Complete initialization
            if hasattr(self._config_manager, 'complete_initialization'):
                self._config_manager.complete_initialization()

            # §4.3: announce the effective (redacted) configuration on the UNS cfg
            # topic - the startup push; the publisher re-announces on every
            # configuration change. Best-effort (publish_now never throws).
            from edgecommons.config.effective_config_publisher import EffectiveConfigPublisher
            from edgecommons.messaging.messaging_client import MessagingClient as _MC2
            self._effective_config_publisher = EffectiveConfigPublisher(
                self._config_manager, _MC2
            )
            self._effective_config_publisher.publish_now()

            # §9.3/§9.4: subscribe the own-device _bcast republish topics on the
            # primary connection so the uns-bridge's reconnect-rehydration broadcast
            # (and a console's explicit republish) gets a jittered, coalesced
            # state/cfg re-announce. Always on (no config surface); best-effort start
            # (a failure disables the listener only).
            from edgecommons.republish_listener import RepublishListener
            self._republish_listener = RepublishListener(
                self._config_manager, _MC2,
                self._heartbeat.publish_state_now,
                self._effective_config_publisher.publish_now,
            )
            self._republish_listener.start()

            # §9.5 (slice S2): subscribe the component's own command inbox - both
            # ecv1/{device}/{component}/cmd/# (component scope) and
            # ecv1/{device}/{component}/+/cmd/# (any instance, D-U28) - on the primary
            # connection and dispatch cmd envelopes by verb - built-ins ping / status / describe /
            # reload-config / get-configuration answer the console out of the box; apps
            # add custom verbs via get_commands().register(). Always on (no config
            # surface); best-effort start (a failure disables the inbox only).
            from edgecommons.command_inbox import CommandInbox
            self._command_inbox = CommandInbox(
                self._config_manager, _MC2,
                self._heartbeat.get_uptime_secs,
                self._config_manager.reload_from_provider,
                self._effective_config_publisher.redacted_effective_config,
                # The built-in `status` verb pulls the SAME provider sample the state
                # keepalive pushes, so the two surfaces cannot disagree.
                self._heartbeat.sample_instance_connectivity,
            )
            # The complete component command surface is installed before the transport
            # subscription can acknowledge and make dispatch externally reachable.
            for configurer in self._command_configurers:
                configurer(self._command_inbox)
            self._command_inbox.start()

            logger.info("EdgeCommons initialized successfully")
            
        except Exception as e:
            logger.error(f"Failed to initialize EdgeCommons: {e}")
            # Tear down whatever was already started (messaging/metrics/heartbeat
            # threads, file watchers) so a failed init does not leak resources.
            # shutdown() is fully defensive, so it is safe on a partial init.
            try:
                self.shutdown()
            except Exception as cleanup_error:
                logger.error(f"Error during cleanup after failed init: {cleanup_error}")
            raise
            
    def _process_args(self, component_name: str, args: List[str], 
                     app_options: Optional[argparse.ArgumentParser]) -> argparse.Namespace:
        """
        Process command line arguments.
        
        Args:
            component_name: The component name
            args: Command line arguments
            app_options: Optional custom argument parser
            
        Returns:
            Parsed arguments namespace
        """
        # The legacy single-axis -m/--mode token is removed (DESIGN-core sec 6.1 / FR-RT-1).
        # Reject it explicitly with guidance to the new flags rather than letting argparse
        # swallow it as an unrecognized option.
        self._reject_legacy_mode_flag(args)

        parser = app_options or argparse.ArgumentParser()

        # Add standard edgecommons arguments. -c/--config defaults to None so the resolver can
        # tell "omitted" (use the platform-profile default) from an explicit value.
        parser.add_argument(
            '-c', '--config',
            nargs='*',
            type=str,
            default=None,
            help='Configuration source. One of: ENV, GG_CONFIG, FILE, CONFIGMAP, SHADOW, '
                 'CONFIG_COMPONENT. CONFIGMAP takes [mount_dir] [key] (defaults /etc/edgecommons, '
                 'config.json). Default: from the resolved platform profile '
                 '(GREENGRASS -> GG_CONFIG, HOST -> FILE, KUBERNETES -> CONFIGMAP)'
        )
        parser.add_argument(
            '--platform',
            type=str,
            default=None,
            help="Deployment platform - 'GREENGRASS', 'HOST', 'KUBERNETES' or 'auto' (default auto)"
        )
        parser.add_argument(
            '--transport',
            nargs='*',
            type=str,
            default=None,
            help="Messaging transport - 'IPC' or 'MQTT <messaging_config_path>' "
                 "(default: derived from the platform)"
        )
        parser.add_argument(
            '-t', '--thing',
            type=str,
            help='Thing name to use (optional)'
        )
        parsed = parser.parse_args(args)

        # Parse the two new runtime axes into resolver inputs (parse-time inputs only).
        platform_flag = self._parse_platform(getattr(parsed, 'platform', None))
        transport_flag = self._parse_transport(parsed)
        config_args = parsed.config if parsed.config else None
        thing_flag = getattr(parsed, 'thing', None)

        # Resolve the platform/transport/config-source/identity from flags > env > profile
        # defaults (DESIGN-core sec 4). KUBERNETES and an illegal IPC combo fail fast here.
        resolved = resolve_profile(
            ResolverInputs(platform_flag, transport_flag, config_args, thing_flag),
            os.environ,
        )
        parsed.platform = resolved.platform
        parsed.transport = resolved.transport
        parsed.config = list(resolved.config_source)
        parsed.identity = resolved.identity

        # Validate the resolved config source token up front rather than failing later.
        valid_sources = {s.value for s in ConfigSource}
        if parsed.config and parsed.config[0].upper() not in valid_sources:
            logger.error(f"Unrecognized config source '{parsed.config[0]}'")
            raise ValueError(
                f"Unrecognized config source '{parsed.config[0]}'. Valid values are "
                f"{', '.join(sorted(valid_sources))}"
            )

        # FR-MSG-1: under the KUBERNETES convention (CONFIGMAP config source + MQTT transport) a
        # single mounted ConfigMap file holds BOTH a `messaging` section (read by the messaging
        # loader) AND the component config (loaded+validated by the CONFIGMAP source afterwards).
        # When no explicit messaging-config path was given on `--transport MQTT`, default it to the
        # resolved CONFIGMAP file path (mount dir + key), using the SAME dir/key the CONFIGMAP source
        # resolves from `-c CONFIGMAP [dir] [key]` (or its profile default). Resolved here from
        # parse-time inputs only (flags/env/profile), BEFORE messaging init — never by reading the
        # ConfigMap through the config source (that runs after messaging). The existing explicit-path
        # behavior is unchanged, and HOST keeps requiring an explicit path (HOST defaults to
        # GG_CONFIG, not CONFIGMAP).
        if (
            parsed.transport == Transport.MQTT
            and getattr(parsed, "standalone_config_path", None) is None
            and parsed.config
            and parsed.config[0].upper() == ConfigSource.CONFIGMAP.value
        ):
            parsed.standalone_config_path = self._configmap_messaging_path(parsed.config)
            logger.info(
                "CONFIGMAP+MQTT: defaulting messaging-config path to the mounted ConfigMap file '%s'",
                parsed.standalone_config_path,
            )

        return parsed

    @staticmethod
    def _configmap_messaging_path(config_args: List[str]) -> str:
        """Resolve the CONFIGMAP file path (mount dir + key) exactly as the CONFIGMAP config source
        does, so one mounted ConfigMap doubles as both the messaging config and the component config
        (FR-MSG-1).

        ``config_args`` is the resolved ``-c`` vector: ``["CONFIGMAP", [mount_dir], [key]]`` (the
        positional dir/key are optional; defaults mirror :class:`ConfigMapConfigManager`).
        """
        # Import here (not at module load) to avoid pulling the config-manager subtree on the hot
        # import path, and to reuse the single source of the CONFIGMAP mount/key defaults.
        from edgecommons.config.manager.configmap_config_manager import (
            DEFAULT_KEY,
            DEFAULT_MOUNT_DIR,
        )

        mount_dir = config_args[1] if len(config_args) > 1 else DEFAULT_MOUNT_DIR
        key = config_args[2] if len(config_args) > 2 else DEFAULT_KEY
        return os.path.join(mount_dir, key)

    @staticmethod
    def _reject_legacy_mode_flag(args: Optional[List[str]]) -> None:
        """Reject the removed -m/--mode flag with guidance to the new axes (DESIGN-core sec 6.1)."""
        if not args:
            return
        for arg in args:
            if arg == "--mode" or arg.startswith("--mode=") or arg.startswith("-m"):
                raise ValueError(
                    "The -m/--mode flag has been removed. Use --platform GREENGRASS|HOST|KUBERNETES "
                    "and --transport IPC|MQTT instead (e.g. '-m STANDALONE <path>' becomes "
                    "'--platform HOST --transport MQTT <path>')."
                )

    @staticmethod
    def _parse_platform(raw: Optional[str]) -> Optional[Platform]:
        """Parse --platform; 'auto' (or absent) yields None so the resolver auto-detects."""
        if raw is None:
            return None
        token = raw.strip()
        if token.lower() == "auto":
            return None
        try:
            return Platform(token.upper())
        except ValueError:
            raise ValueError(
                f"Unknown platform '{raw}'. Valid: GREENGRASS, HOST, KUBERNETES, auto."
            )

    @staticmethod
    def _parse_transport(parsed: argparse.Namespace) -> Optional[Transport]:
        """Parse --transport [IPC|MQTT] <optional messaging-config path>.

        Absent yields None so the resolver derives the transport from the platform. The optional
        second value (the MQTT messaging-config path) is stashed on the namespace as
        ``standalone_config_path``.
        """
        parsed.standalone_config_path = None
        transport_args = getattr(parsed, 'transport', None)
        if not transport_args:
            return None
        if len(transport_args) > 1:
            parsed.standalone_config_path = transport_args[1]
        try:
            return Transport(transport_args[0].upper())
        except ValueError:
            raise ValueError(
                f"Unknown transport '{transport_args[0]}'. Valid: IPC, MQTT."
            )
        
    def _init_config_manager(self, component_name: str, parsed_args: argparse.Namespace) -> None:
        """
        Initialize the configuration manager.
        
        Args:
            component_name: The component name
            parsed_args: Parsed command line arguments
        """
        # Use config manager builder to create appropriate manager
        self._config_manager = ConfigManagerBuilder.build(
            parsed_args,
            component_name,
            candidate_validators=self._configuration_validators,
            validation_timeout_secs=self._configuration_validation_timeout,
        )
        
    def _init_messaging(self, parsed_args: argparse.Namespace, receive_own_messages: bool) -> None:
        """
        Initialize the messaging client.
        
        Args:
            parsed_args: Parsed command line arguments
            receive_own_messages: Whether to receive own messages
        """
        # Import here to avoid circular imports
        from edgecommons.messaging.messaging_client import MessagingClient

        # The MQTT messaging-config path was stashed on the namespace during transport parsing
        # (--transport MQTT <path>); the IPC transport ignores it.
        standalone_config_path = getattr(parsed_args, 'standalone_config_path', None)

        MessagingClient.init(parsed_args, standalone_config_path, receive_own_messages)
        
    def _init_metrics(self) -> None:
        """Initialize the metric emitter."""
        # Import here to avoid circular imports
        from edgecommons.metrics.metric_emitter import MetricEmitter

        MetricEmitter.init(self._config_manager)

    def _init_logs(self, messaging_client) -> None:
        """Configure the library-owned UNS log publisher and hot-reload listener."""
        self._logs.configure(self._config_manager, messaging_client)
        self._config_manager.add_config_change_listener(self._logs)

    def set_instance_connectivity_provider(self, provider) -> None:
        """Register the component's per-instance connectivity provider — the overridable
        surface for reporting connectivity AT THE INSTANCE LEVEL (each configured
        connection's health) in the component's ``state`` keepalive's ``instances`` array,
        without minting a separate UNS instance per connection (data + lifecycle stay at
        component scope — no instance token, D-U28). A reference adapter maps each connection to its reachability: OPC UA
        server session / Modbus slave / file-replicator source directory. The same sample
        answers the built-in ``status`` command verb when pulled, so a component supplies
        the data once and the library serves it on both surfaces. No-op when the heartbeat
        is not wired (test bring-up). Pass ``None`` to stop reporting.

        :param provider: a zero-arg callable returning a list of
            :class:`~edgecommons.heartbeat.instance_connectivity.InstanceConnectivity`, or
            ``None`` to clear.
        """
        if self._heartbeat is not None:
            self._heartbeat.set_instance_connectivity_provider(provider)

    def _init_heartbeat(self) -> None:
        """Initialize the heartbeat system, wiring it to the concrete subsystems."""
        # Import here to avoid circular imports
        from edgecommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat
        from edgecommons.messaging.messaging_client import MessagingClient
        from edgecommons.metrics.metric_emitter import MetricEmitter

        self._heartbeat = EnhancedHeartbeat(self._config_manager)
        # MessagingClient / MetricEmitter expose their operations as static methods,
        # so the classes themselves serve as the messaging/metric handles.
        self._heartbeat.set_messaging_service(MessagingClient)
        self._heartbeat.set_metric_service(MetricEmitter)
            
    def _init_streaming(self) -> None:
        """Open telemetry streams from the ``streaming`` config section (if any), resolving
        templates, and start the stats->metrics bridge. No-op when the section is absent."""
        import json as _json

        full_config = self._config_manager.get_full_config()
        streaming = full_config.get("streaming") if isinstance(full_config, dict) else None
        if not isinstance(streaming, dict):
            return

        from edgecommons.streaming import StreamMetricsBridge, StreamService

        # Resolve {ThingName} etc. across the streaming section (buffer paths, Kinesis stream names).
        streaming_json = self._config_manager.resolve_template(_json.dumps(streaming))

        # Resolve {"$secret": ...} refs from the vault BEFORE opening streaming (closes
        # TELEMETRY_STREAMING.md §7), on a COPY so the public config snapshot is never mutated and
        # the secret never lands in the logged/templated config. Mirrors the Rust build() order.
        if self._credentials is not None:
            from edgecommons.credentials import resolve_secret_refs

            streaming_dict = _json.loads(streaming_json)
            resolve_secret_refs(streaming_dict, self._credentials)
            streaming_json = _json.dumps(streaming_dict)

        self._streams = StreamService.open(streaming_json)
        names = StreamService.stream_names(streaming_json)
        if names:
            self._stream_metrics = StreamMetricsBridge(self._config_manager, self._streams, names)
        logger.info(f"Telemetry streaming initialized with {len(names)} stream(s)")

    def get_streams(self):
        """
        Get the telemetry streaming service, or ``None`` if the component config has no
        ``streaming`` section. Obtain a stream with ``service.stream(name)`` and append durable
        records. Mirrors Java's getStreams() / Rust's gg.streams().
        """
        return self._streams

    def _init_credentials(self) -> None:
        """Open the local vault from the ``credentials`` config section (if any), resolving path
        templates. No-op when the section is absent."""
        import json as _json

        full_config = self._config_manager.get_full_config()
        credentials = full_config.get("credentials") if isinstance(full_config, dict) else None
        if not isinstance(credentials, dict):
            return

        from edgecommons.credentials import CredentialMetricsBridge, open_from_config
        from edgecommons.platform.resolver import profile_credentials_key_provider

        # Resolve {ThingName}/{ComponentFullName} in the vault path(s) before opening.
        resolved = _json.loads(self._config_manager.resolve_template(_json.dumps(credentials)))
        # Transparently namespace every key by <thingName>/<componentName> (collision-free across
        # components/devices).
        namespace = f"{self._config_manager.get_thing_name()}/{self._config_manager.get_component_full_name()}"
        # FR-CRED-6 / FR-RT-3: the platform-profile default vault key provider (env on KUBERNETES) is
        # the middle precedence tier, applied only when keyProvider.type is absent. The resolved
        # platform is read from the config manager (threaded by the builder, same as logging/health/
        # metric defaults) — no new resolver->ConfigManager dependency. This does NOT enable
        # credentials (gated above by the credentials section's presence); it only changes the
        # DEFAULT provider type when credentials is configured without an explicit keyProvider.type.
        default_key_provider = profile_credentials_key_provider(self._config_manager.get_platform())
        self._credentials = open_from_config(
            resolved, namespace, default_key_provider=default_key_provider
        )
        # Bridge non-sensitive credential stats into the metric targets (never emits secret values).
        self._credential_metrics = CredentialMetricsBridge(self._credentials)
        logger.info("Credentials vault initialized")

    def get_credentials(self):
        """
        Get the credential service, or ``None`` if the component config has no ``credentials``
        section. Mirrors Java/TS getCredentials() / Rust's gg.credentials().
        """
        return self._credentials

    def _init_parameters(self) -> None:
        """Open the parameter service from the ``parameters`` config section (if any), resolving path
        templates. No-op when the section is absent."""
        import json as _json

        full_config = self._config_manager.get_full_config()
        parameters = full_config.get("parameters") if isinstance(full_config, dict) else None
        if not isinstance(parameters, dict):
            return

        from edgecommons.parameters import open_from_config

        # Resolve {ThingName}/{ComponentFullName} in the cache path(s) before opening. Parameter
        # keys are NOT namespaced (the cache path is already per-component templated), matching Rust.
        resolved = _json.loads(self._config_manager.resolve_template(_json.dumps(parameters)))
        self._parameters = open_from_config(resolved)
        logger.info("Parameters service initialized")

    def get_parameters(self):
        """
        Get the parameter service, or ``None`` if the component config has no ``parameters``
        section. Mirrors Java/TS getParameters() / Rust's gg.parameters().
        """
        return self._parameters

    def _init_health(self, parsed_args: argparse.Namespace) -> None:
        """Build the readiness state and (when enabled) start the HTTP health server (FR-HB-1).

        Readiness is always created so :meth:`set_ready` is usable even when the server is off. The
        server is enabled by the FR-RT-3 precedence: explicit ``health.enabled`` config ▸ on by
        default on KUBERNETES ▸ off. The resolved platform is read from the namespace (set by
        ``_process_args``), reusing the same threading as the logging default — no new
        resolver->ConfigManager dependency.
        """
        from edgecommons.health import HealthServer, ReadinessState
        from edgecommons.messaging.messaging_client import MessagingClient
        from edgecommons.platform.resolver import profile_health_enabled

        # Readiness queries the live messaging connection on each check; if messaging is not wired
        # MessagingClient.connected() returns False (not ready).
        self._readiness = ReadinessState(
            lambda: MessagingClient.connected(),
            initial_ready=getattr(self, "_initial_ready", True),
            required_ready_fn=self._command_plane_active,
        )

        health_config = self._config_manager.get_health_config()
        platform = getattr(parsed_args, "platform", None)
        if not isinstance(platform, Platform):
            platform = None

        explicit = health_config.enabled  # Optional[bool]; None => use the platform default
        enabled = explicit if explicit is not None else profile_health_enabled(platform)
        if not enabled:
            logger.debug(
                "Health server disabled (explicit=%s, platform=%s)",
                explicit,
                platform.value if platform else None,
            )
            return

        try:
            self._health_server = HealthServer(health_config, self._readiness)
            self._health_server.start()
        except Exception as e:
            # A bind failure (e.g. port in use) must not abort component startup; log and continue.
            logger.error(f"Failed to start health server: {e}")
            self._health_server = None

    def set_ready(self, ready: bool) -> None:
        """Set the app-controlled readiness flag consulted by ``/readyz`` and ``/startupz`` (FR-HB-1).

        Defaults to ``True`` (a component is ready once messaging connects). An app that must finish
        its own setup (e.g. confirm required subscriptions) before serving traffic can call
        ``gg.set_ready(False)`` early and ``gg.set_ready(True)`` once ready. No-op if the readiness
        state was never built (a fully failed init). Mirrors Java/TS ``setReady`` and Rust
        ``set_ready``.
        """
        if self._readiness is not None:
            self._readiness.set_ready(ready)

    def _command_plane_active(self) -> bool:
        """Whether the acknowledged command-inbox generation is dispatch-capable."""

        if not hasattr(self, "_command_inbox"):
            # Bare-object test/subclass bring-up predating command lifecycle wiring.
            return True
        inbox = self._command_inbox
        if inbox is None:
            return False
        try:
            return inbox.startup_status().state.value == "ACTIVE"
        except Exception:  # noqa: BLE001 - readiness fails closed
            return False

    def _install_signal_handlers(self) -> None:
        """Wire SIGTERM **and SIGINT** to the graceful-shutdown path (FR-HB-2).

        Both termination signals route to :meth:`_handle_termination_signal`, at parity with the
        Java/Rust/TS libraries (Java's JVM hook fires on SIGTERM+SIGINT, TS wires both
        ``process.on`` signals, Rust awaits SIGTERM and Ctrl-C). SIGTERM is what orchestrators send
        (Kubernetes, ``docker stop``, the Nucleus); SIGINT is interactive ``Ctrl-C`` on a local/host
        run.

        Installed only on the main thread — ``signal.signal`` raises ``ValueError`` off the main
        thread (e.g. when a component is embedded in a worker thread or under some test runners), in
        which case the app keeps responsibility for calling :meth:`shutdown`. The previous handler
        for each signal is saved and restored on shutdown so the library does not permanently hijack
        signals.
        """
        if threading.current_thread() is not threading.main_thread():
            logger.debug("Not on the main thread; skipping library SIGTERM/SIGINT handler install")
            return
        try:
            self._prev_sigterm_handler = signal.signal(signal.SIGTERM, self._handle_termination_signal)
            self._sigterm_installed = True
            logger.debug("Installed library SIGTERM handler for graceful shutdown")
        except (ValueError, OSError, RuntimeError) as e:
            logger.warning(f"Could not install SIGTERM handler (app must call shutdown itself): {e}")
        try:
            self._prev_sigint_handler = signal.signal(signal.SIGINT, self._handle_termination_signal)
            self._sigint_installed = True
            logger.debug("Installed library SIGINT handler for graceful shutdown")
        except (ValueError, OSError, RuntimeError) as e:
            logger.warning(f"Could not install SIGINT handler (app must call shutdown itself): {e}")

    def _handle_termination_signal(self, signum, frame) -> None:
        """SIGTERM/SIGINT handler: flip readiness to 503, run the idempotent shutdown, then exit 0."""
        logger.info(f"Received signal {signum}; beginning graceful shutdown")
        # Flip /readyz to 503 immediately, before draining (FR-HB-2 acceptance).
        if self._readiness is not None:
            self._readiness.set_shutting_down()
        try:
            self.shutdown()
        finally:
            sys.exit(0)

    def __enter__(self) -> "EdgeCommons":
        """Support `with EdgeCommonsBuilder...build() as gg:` so callers get
        deterministic shutdown without a manual try/finally."""
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> bool:
        self.shutdown()
        return False

    def get_config_manager(self) -> ConfigManager:
        """
        Get the configuration manager instance.
        
        Returns:
            The configuration manager
            
        Raises:
            RuntimeError: If not properly initialized
        """
        if self._config_manager is None:
            raise RuntimeError("EdgeCommons not properly initialized")
        return self._config_manager
        
    def get_messaging(self):
        """
        Get the messaging handle (the MessagingClient class, whose operations are
        static). Mirrors Java's getMessaging() / Rust's messaging() accessor.

        Returns:
            The MessagingClient class
        """
        from edgecommons.messaging.messaging_client import MessagingClient
        return MessagingClient

    def get_metrics(self):
        """
        Get the metrics handle (the MetricEmitter class, whose operations are
        static). Mirrors Java's getMetrics() / Rust's metrics() accessor.

        Returns:
            The MetricEmitter class
        """
        from edgecommons.metrics.metric_emitter import MetricEmitter
        return MetricEmitter

    def get_commands(self):
        """
        Get the command-inbox facade — the minimal ``commands()`` surface
        (DESIGN-uns §9.5): register custom command verbs with
        ``get_commands().register(verb, handler)``; the built-in verbs (``ping``,
        ``reload-config``, ``get-configuration``) are registered by the library and
        cannot be shadowed. Mirrors Java's ``getCommands()`` / Rust's/TS's
        ``gg.commands()``. ``None`` on a mock/subclass bring-up that never ran
        ``__init__``.

        Returns:
            The :class:`~edgecommons.command_inbox.CommandInbox` facade
        """
        return self._command_inbox

    def uns(self):
        """The UNS topic builder + validator bound to this component's resolved
        identity (component scope — no instance token, D-U28) and its
        ``topic.includeRoot`` setting (UNS-CANONICAL-DESIGN §2). For instance-scoped topics use
        ``instance(id).uns()``. Mirrors Java's ``getUns()``.

        :raises RuntimeError: when called before initialization completes (no resolved
            component identity yet)
        """
        from edgecommons.uns import Uns

        if self._uns is None:
            cm = self._require_resolved_identity()
            self._uns = Uns(cm.get_component_identity(), cm.is_topic_include_root())
        return self._uns

    def instance(self, instance_id: str):
        """The instance-scoped handle for an instance token (UNS-CANONICAL-DESIGN §3,
        D-U3): a :class:`~edgecommons.edgecommons_instance.EdgeCommonsInstance` whose ``uns()`` mints
        topics with — and whose ``new_message(...)`` stamps envelopes with — this
        instance token. The token is validated against the §2.2 token rule; handles
        are cached per id, so repeated calls return the same object. The id is
        deliberately NOT verified against the configured ``component.instances[]``
        (instances may be created dynamically) — an unknown id is only logged at DEBUG
        as a diagnostic aid.

        :raises edgecommons.uns.UnsValidationError: when the token violates the §2.2
            token rule
        :raises RuntimeError: when called before initialization completes
        """
        from edgecommons.edgecommons_instance import EdgeCommonsInstance
        from edgecommons.messaging.messaging_client import MessagingClient
        from edgecommons.uns import Uns

        Uns.check_token(instance_id, "instance id")
        cm = self._require_resolved_identity()
        handle = self._instance_handles.get(instance_id)
        if handle is None:
            configured = cm.get_instance_ids()
            if not configured or instance_id not in configured:
                logger.debug(
                    "instance('%s'): id is not among the configured"
                    " component.instances[] ids %s - creating a dynamic instance handle",
                    instance_id,
                    configured,
                )
            handle = EdgeCommonsInstance(instance_id, cm, cm.is_topic_include_root(),
                               MessagingClient, self._stream_sink(), self._clock)
            self._instance_handles[instance_id] = handle
        return handle

    def _stream_sink(self):
        """The stream seam the ``data()`` facade composes for a ``stream:<name>``
        channel (DESIGN-class-facades §4): binds ``get_streams().stream(name).append(...)``
        when streaming is configured, else ``None`` so the facade falls a stream route
        back to a LOCAL publish.

        :return: the stream sink callable, or ``None`` when no ``streaming`` section is
            configured
        """
        streams = self._streams
        if streams is None:
            return None

        def _sink(stream_name, partition_key, timestamp_ms, payload):
            streams.stream(stream_name).append(partition_key, timestamp_ms, payload)

        return _sink

    def data(self):
        """The ``data()`` publish facade at **component scope** (D-U28: no instance
        token) — for an instance-scoped facade use ``instance(id).data()``.
        Builds/validates the ``SouthboundSignalUpdate`` body. Mirrors Java's
        ``getData()``.

        :raises RuntimeError: when called before initialization completes
        """
        return self._component_scope().data()

    def events(self):
        """The ``events()`` publish facade at **component scope** (D-U28: no instance
        token) — for an instance-scoped facade use ``instance(id).events()``. Operator
        events & alarms on the ``evt`` class. Mirrors Java's ``getEvents()``.

        :raises RuntimeError: when called before initialization completes
        """
        return self._component_scope().events()

    def app(self):
        """The ``app()`` publish facade at **component scope** (D-U28: no instance
        token) — for an instance-scoped facade use ``instance(id).app()``. Free-form
        inter-component pub/sub on the ``app`` class. Mirrors Java's ``getApp()``.

        :raises RuntimeError: when called before initialization completes
        """
        return self._component_scope().app()

    def _component_scope(self):
        """The component-scope handle (D-U28: no instance token) backing
        :meth:`data`, :meth:`events`, and :meth:`app`. Lazily built and cached,
        mirroring :meth:`uns`."""
        from edgecommons.edgecommons_instance import EdgeCommonsInstance
        from edgecommons.messaging.messaging_client import MessagingClient

        if self._component_handle is None:
            cm = self._require_resolved_identity()
            self._component_handle = EdgeCommonsInstance(
                None, cm, cm.is_topic_include_root(), MessagingClient,
                self._stream_sink(), self._clock,
            )
        return self._component_handle

    def logs(self):
        """The library-owned UNS ``log`` publisher facade for this component."""
        return self._logs

    def _require_resolved_identity(self) -> ConfigManager:
        """Guards the UNS accessors: they need the config manager and its resolved
        component identity, which exist only after init has constructed the
        ConfigManager."""
        if self._config_manager is None or self._config_manager.get_component_identity() is None:
            raise RuntimeError(
                "EdgeCommons is not initialized: the component configuration (and its"
                " resolved UNS identity) is not available yet"
            )
        return self._config_manager


    def shutdown(self) -> None:
        """
        Shutdown EdgeCommons and clean up resources.

        Each subsystem is closed independently so a failure in one does not leave
        the others leaking: heartbeat -> metrics -> messaging -> config (matching
        the Java shutdown order).
        """
        from edgecommons.messaging.messaging_client import MessagingClient
        from edgecommons.metrics.metric_emitter import MetricEmitter

        # Flip /readyz to 503 first so a probe sees "not ready" the instant shutdown begins, whether
        # this was reached via SIGTERM or a direct shutdown() call (FR-HB-2). Idempotent.
        if self._readiness is not None:
            self._readiness.set_shutting_down()

        try:
            # Unsubscribe the _bcast republish topics while messaging is still up (the
            # unsubscribe-before-exit rule) and stop reacting to republish broadcasts
            # mid-teardown.
            if self._republish_listener is not None:
                self._republish_listener.close()
        except Exception as e:
            logger.error(f"Error closing republish listener during shutdown: {e}")

        try:
            # Unsubscribe the command inbox while messaging is still up (same rule)
            # and stop dispatching command verbs mid-teardown.
            if self._command_inbox is not None:
                self._command_inbox.close()
        except Exception as e:
            logger.error(f"Error closing command inbox during shutdown: {e}")

        try:
            # Stop the streaming stats bridge + close the native service (flush + stop engines).
            if self._stream_metrics is not None:
                self._stream_metrics.close()
        except Exception as e:
            logger.error(f"Error stopping stream metrics during shutdown: {e}")
        try:
            if self._streams is not None:
                self._streams.close()
        except Exception as e:
            logger.error(f"Error closing streams during shutdown: {e}")

        try:
            # Stop the credential stats bridge + the central sync thread (if any).
            if self._credential_metrics is not None:
                self._credential_metrics.close()
        except Exception as e:
            logger.error(f"Error stopping credential metrics during shutdown: {e}")
        try:
            if self._credentials is not None and getattr(self._credentials, "_sync", None) is not None:
                self._credentials._sync.close()
        except Exception as e:
            logger.error(f"Error closing credential sync during shutdown: {e}")

        try:
            # Stop the parameter background refresh thread (if any).
            if self._parameters is not None and hasattr(self._parameters, "close"):
                self._parameters.close()
        except Exception as e:
            logger.error(f"Error closing parameters during shutdown: {e}")

        try:
            # Stop the heartbeat first so it stops publishing/emitting.
            if self._heartbeat and hasattr(self._heartbeat, 'stop'):
                self._heartbeat.stop()
        except Exception as e:
            logger.error(f"Error stopping heartbeat during shutdown: {e}")

        try:
            # Flush + stop the metric emitter's target thread.
            MetricEmitter.shutdown()
        except Exception as e:
            logger.error(f"Error shutting down metrics during shutdown: {e}")

        try:
            if self._logs is not None:
                self._logs.close()
        except Exception as e:
            logger.error(f"Error shutting down log bus during shutdown: {e}")

        try:
            MessagingClient.shutdown()
        except Exception as e:
            logger.error(f"Error shutting down messaging during shutdown: {e}")

        try:
            # Stop the config manager's file-watcher thread (if any).
            if self._config_manager is not None:
                self._config_manager.close()
        except Exception as e:
            logger.error(f"Error closing config manager during shutdown: {e}")

        try:
            # Stop the HTTP health server last (it served 503 during the drain above) and release
            # the socket. Idempotent + bounded (joins the daemon thread briefly).
            if self._health_server is not None:
                self._health_server.stop()
                self._health_server = None
        except Exception as e:
            logger.error(f"Error stopping health server during shutdown: {e}")

        # Restore the previous SIGTERM/SIGINT handlers so the library does not permanently hijack
        # signals (matters for tests and embedding apps). Only restore a signal we installed, and
        # only on the main thread (signal.signal raises elsewhere).
        if self._sigterm_installed and threading.current_thread() is threading.main_thread():
            try:
                signal.signal(signal.SIGTERM, self._prev_sigterm_handler or signal.SIG_DFL)
            except (ValueError, OSError, RuntimeError) as e:
                logger.debug(f"Could not restore previous SIGTERM handler: {e}")
            finally:
                self._sigterm_installed = False
        if self._sigint_installed and threading.current_thread() is threading.main_thread():
            try:
                signal.signal(signal.SIGINT, self._prev_sigint_handler or signal.SIG_DFL)
            except (ValueError, OSError, RuntimeError) as e:
                logger.debug(f"Could not restore previous SIGINT handler: {e}")
            finally:
                self._sigint_installed = False

        logger.info("EdgeCommons shutdown completed")
