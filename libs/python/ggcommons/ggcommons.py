"""
GGCommons main class with builder support.

This module provides the main GGCommons class. Component code accesses the
underlying subsystems through typed accessors (get_config_manager / get_messaging
/ get_metrics) rather than a service registry — matching the Java and Rust
libraries, which depend on the concrete ConfigManager / MessagingClient /
MetricEmitter directly.
"""

import argparse
import logging
import os
from enum import Enum
from typing import Optional, List
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.config_manager_builder import ConfigManagerBuilder
from ggcommons.platform import (
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
    SHADOW = "SHADOW"
    CONFIG_COMPONENT = "CONFIG_COMPONENT"


class GGCommons:
    """
    Main entry point for the GGCommons framework with enhanced features.
    
    Provides dependency injection, service registry, and improved initialization.
    """

    def __init__(self, component_name: str, args: List[str], 
                 app_options: Optional[argparse.ArgumentParser] = None,
                 receive_own_messages: bool = True):
        """
        Initialize GGCommons with enhanced features.
        
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
        self._config_manager: Optional[ConfigManager] = None
        self._heartbeat = None
        self._streams = None
        self._stream_metrics = None
        self._credentials = None
        self._credential_metrics = None
        self._parameters = None

        try:
            # Process command line arguments
            parsed_args = self._process_args(component_name, args, app_options)

            # Initialize messaging FIRST: the GG_CONFIG / SHADOW / CONFIG_COMPONENT
            # config sources load the component configuration over messaging, so the
            # MessagingClient must be available before the config manager is built.
            self._init_messaging(parsed_args, receive_own_messages)

            # Initialize configuration manager
            self._init_config_manager(component_name, parsed_args)

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

            # Complete initialization
            if hasattr(self._config_manager, 'complete_initialization'):
                self._config_manager.complete_initialization()
                
            logger.info("GGCommons initialized successfully")
            
        except Exception as e:
            logger.error(f"Failed to initialize GGCommons: {e}")
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

        # Add standard ggcommons arguments. -c/--config defaults to None so the resolver can
        # tell "omitted" (use the platform-profile default) from an explicit value.
        parser.add_argument(
            '-c', '--config',
            nargs='*',
            type=str,
            default=None,
            help='Configuration source. One of: ENV, GG_CONFIG, FILE, SHADOW, CONFIG_COMPONENT. '
                 'Default: from the resolved platform profile (GG_CONFIG)'
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

        return parsed

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
        self._config_manager = ConfigManagerBuilder.build(parsed_args, component_name)
        
    def _init_messaging(self, parsed_args: argparse.Namespace, receive_own_messages: bool) -> None:
        """
        Initialize the messaging client.
        
        Args:
            parsed_args: Parsed command line arguments
            receive_own_messages: Whether to receive own messages
        """
        # Import here to avoid circular imports
        from ggcommons.messaging.messaging_client import MessagingClient

        # The MQTT messaging-config path was stashed on the namespace during transport parsing
        # (--transport MQTT <path>); the IPC transport ignores it.
        standalone_config_path = getattr(parsed_args, 'standalone_config_path', None)

        MessagingClient.init(parsed_args, standalone_config_path, receive_own_messages)
        
    def _init_metrics(self) -> None:
        """Initialize the metric emitter."""
        # Import here to avoid circular imports
        from ggcommons.metrics.metric_emitter import MetricEmitter

        MetricEmitter.init(self._config_manager)

    def _init_heartbeat(self) -> None:
        """Initialize the heartbeat system, wiring it to the concrete subsystems."""
        # Import here to avoid circular imports
        from ggcommons.heartbeat.enhanced_heartbeat import EnhancedHeartbeat
        from ggcommons.messaging.messaging_client import MessagingClient
        from ggcommons.metrics.metric_emitter import MetricEmitter

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

        from ggcommons.streaming import StreamMetricsBridge, StreamService

        # Resolve {ThingName} etc. across the streaming section (buffer paths, Kinesis stream names).
        streaming_json = self._config_manager.resolve_template(_json.dumps(streaming))

        # Resolve {"$secret": ...} refs from the vault BEFORE opening streaming (closes
        # TELEMETRY_STREAMING.md §7), on a COPY so the public config snapshot is never mutated and
        # the secret never lands in the logged/templated config. Mirrors the Rust build() order.
        if self._credentials is not None:
            from ggcommons.credentials import resolve_secret_refs

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

        from ggcommons.credentials import CredentialMetricsBridge, open_from_config

        # Resolve {ThingName}/{ComponentFullName} in the vault path(s) before opening.
        resolved = _json.loads(self._config_manager.resolve_template(_json.dumps(credentials)))
        # Transparently namespace every key by <thingName>/<componentName> (collision-free across
        # components/devices).
        namespace = f"{self._config_manager.get_thing_name()}/{self._config_manager.get_component_full_name()}"
        self._credentials = open_from_config(resolved, namespace)
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

        from ggcommons.parameters import open_from_config

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

    def __enter__(self) -> "GGCommons":
        """Support `with GGCommonsBuilder...build() as gg:` so callers get
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
            raise RuntimeError("GGCommons not properly initialized")
        return self._config_manager
        
    def get_messaging(self):
        """
        Get the messaging handle (the MessagingClient class, whose operations are
        static). Mirrors Java's getMessaging() / Rust's messaging() accessor.

        Returns:
            The MessagingClient class
        """
        from ggcommons.messaging.messaging_client import MessagingClient
        return MessagingClient

    def get_metrics(self):
        """
        Get the metrics handle (the MetricEmitter class, whose operations are
        static). Mirrors Java's getMetrics() / Rust's metrics() accessor.

        Returns:
            The MetricEmitter class
        """
        from ggcommons.metrics.metric_emitter import MetricEmitter
        return MetricEmitter


    def shutdown(self) -> None:
        """
        Shutdown GGCommons and clean up resources.

        Each subsystem is closed independently so a failure in one does not leave
        the others leaking: heartbeat -> metrics -> messaging -> config (matching
        the Java shutdown order).
        """
        from ggcommons.messaging.messaging_client import MessagingClient
        from ggcommons.metrics.metric_emitter import MetricEmitter

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
            MessagingClient.shutdown()
        except Exception as e:
            logger.error(f"Error shutting down messaging during shutdown: {e}")

        try:
            # Stop the config manager's file-watcher thread (if any).
            if self._config_manager is not None:
                self._config_manager.close()
        except Exception as e:
            logger.error(f"Error closing config manager during shutdown: {e}")

        logger.info("GGCommons shutdown completed")