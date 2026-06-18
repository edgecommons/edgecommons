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
from typing import Optional, List
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.config_manager_builder import ConfigManagerBuilder

logger = logging.getLogger(__name__)


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
        parser = app_options or argparse.ArgumentParser()
        
        # Add standard ggcommons arguments
        parser.add_argument(
            '-c', '--config',
            nargs='*',
            type=str,
            default=['GG_CONFIG'],
            help='Configuration source. One of: ENV, GG_CONFIG, FILE, SHADOW, CONFIG_COMPONENT'
        )
        parser.add_argument(
            '-m', '--mode',
            nargs='*',
            type=str,
            help='Runtime mode - GREENGRASS (default) or STANDALONE <config_file_path>'
        )
        parser.add_argument(
            '-t', '--thing',
            type=str,
            help='Thing name to use (optional)'
        )
        
        parsed = parser.parse_args(args)
        
        # Process mode argument to match Java behavior
        if not hasattr(parsed, 'mode') or not parsed.mode:
            parsed.mode = ['GREENGRASS']

        mode_name = parsed.mode[0].upper()
        # Validate STANDALONE mode has config path
        if mode_name == 'STANDALONE':
            if len(parsed.mode) < 2:
                logger.error("STANDALONE mode requires config file path")
                raise ValueError("STANDALONE mode requires config file path")
        elif mode_name != 'GREENGRASS':
            # Reject unknown modes instead of silently treating them as GREENGRASS.
            logger.error(f"Unknown mode '{parsed.mode[0]}'")
            raise ValueError(
                f"Unknown mode '{parsed.mode[0]}'. Valid values are 'GREENGRASS' and 'STANDALONE'"
            )

        # Validate the config source token up front rather than failing later.
        valid_sources = {'FILE', 'ENV', 'GG_CONFIG', 'SHADOW', 'CONFIG_COMPONENT'}
        if parsed.config and parsed.config[0].upper() not in valid_sources:
            logger.error(f"Unrecognized config source '{parsed.config[0]}'")
            raise ValueError(
                f"Unrecognized config source '{parsed.config[0]}'. Valid values are "
                f"{', '.join(sorted(valid_sources))}"
            )

        return parsed
        
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
        
        # Determine standalone config path
        standalone_config_path = None
        if hasattr(parsed_args, 'mode') and parsed_args.mode:
            if len(parsed_args.mode) > 1 and parsed_args.mode[0].upper() == 'STANDALONE':
                standalone_config_path = parsed_args.mode[1]
        
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