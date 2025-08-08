"""
Enhanced heartbeat system with service injection and improved lifecycle management.

This module provides an enhanced heartbeat implementation with dependency injection,
better error handling, and improved timer management.
"""

import logging
import threading
import time
from typing import Optional, Dict, Any
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.heartbeat.heartbeat_monitor import HeartbeatMonitor
from ggcommons.interfaces import IMessagingService, IMetricService, IConfigurationService

logger = logging.getLogger(__name__)


class EnhancedHeartbeat(ConfigurationChangeListener):
    """
    Enhanced heartbeat implementation with service injection and improved lifecycle management.
    
    Features:
    - Dependency injection for messaging and metric services
    - Better timer lifecycle management
    - Enhanced error handling and recovery
    - Thread-safe configuration updates
    - Proper resource cleanup
    """
    
    MESSAGE_NAME = "heartbeat"
    MESSAGE_VERSION = "1.0.0"
    
    def __init__(self, config_service: IConfigurationService):
        """
        Initialize enhanced heartbeat with configuration service.
        
        Args:
            config_service: Configuration service for accessing heartbeat config
            
        Raises:
            ValueError: If config_service is None
        """
        if config_service is None:
            raise ValueError("Configuration service cannot be None")
            
        super().__init__()
        self._config_service = config_service
        self._messaging_service: Optional[IMessagingService] = None
        self._metric_service: Optional[IMetricService] = None
        
        self._heartbeat_timer: Optional[threading.Timer] = None
        self._heartbeat_monitor: Optional[HeartbeatMonitor] = None
        self._timer_lock = threading.RLock()
        self._running = False
        
        # Register for configuration changes
        self._config_service.add_config_change_listener(self)
        
        # Initialize heartbeat
        self._initialize_heartbeat()
        
    def set_messaging_service(self, messaging_service: IMessagingService) -> None:
        """
        Set the messaging service for dependency injection.
        
        Args:
            messaging_service: The messaging service implementation
        """
        self._messaging_service = messaging_service
        logger.debug("Messaging service injected into heartbeat")
        
    def set_metric_service(self, metric_service: IMetricService) -> None:
        """
        Set the metric service for dependency injection.
        
        Args:
            metric_service: The metric service implementation
        """
        self._metric_service = metric_service
        logger.debug("Metric service injected into heartbeat")
        
    def _initialize_heartbeat(self) -> None:
        """Initialize the heartbeat system with current configuration."""
        try:
            # Get heartbeat configuration
            heartbeat_config = self._get_heartbeat_config()
            if heartbeat_config is None:
                logger.warning("No heartbeat configuration found, using defaults")
                return
                
            # Create heartbeat monitor
            self._heartbeat_monitor = HeartbeatMonitor(heartbeat_config)
            
            # Define metrics
            self._define_heartbeat_metric()
            
            # Start heartbeat timer
            self._start_heartbeat_timer()
            
            logger.info(f"Enhanced heartbeat initialized with {heartbeat_config.get('intervalSecs', 30)}s interval")
            
        except Exception as e:
            logger.error(f"Failed to initialize enhanced heartbeat: {e}")
            raise
            
    def _get_heartbeat_config(self) -> Optional[Dict[str, Any]]:
        """
        Get heartbeat configuration from config service.
        
        Returns:
            Heartbeat configuration dictionary or None
        """
        try:
            full_config = self._config_service.get_full_config()
            return full_config.get('heartbeat')
        except Exception as e:
            logger.error(f"Failed to get heartbeat configuration: {e}")
            return None
            
    def _define_heartbeat_metric(self) -> None:
        """Define the heartbeat metric for emission."""
        try:
            if self._metric_service is None:
                logger.debug("No metric service available, skipping metric definition")
                return
                
            # Import here to avoid circular imports
            from ggcommons.metrics.metric_builder import MetricBuilder
            
            # Get configuration for metric definition
            heartbeat_config = self._get_heartbeat_config()
            interval_secs = heartbeat_config.get('intervalSecs', 30) if heartbeat_config else 30
            storage_resolution = 1 if interval_secs < 60 else 60
            
            # Build heartbeat metric
            metric = MetricBuilder.create("heartbeat") \\\n                .with_namespace("GGCommons/Heartbeat") \\\n                .with_thing_name(self._config_service.get_thing_name()) \\\n                .with_component_name(self._config_service.get_component_name()) \\\n                .add_measure("disk_total", "Gigabytes", storage_resolution) \\\n                .add_measure("disk_used", "Gigabytes", storage_resolution) \\\n                .add_measure("disk_free", "Gigabytes", storage_resolution) \\\n                .add_measure("cpu_usage", "Percent", storage_resolution) \\\n                .add_measure("memory_usage", "Megabytes", storage_resolution) \\\n                .add_measure("threads", "Count", storage_resolution) \\\n                .add_measure("files", "Count", storage_resolution) \\\n                .add_measure("fds", "Count", storage_resolution) \\\n                .build()
                
            self._metric_service.define_metric(metric)
            logger.debug("Heartbeat metric defined successfully")
            
        except Exception as e:
            logger.error(f"Failed to define heartbeat metric: {e}")
            
    def _start_heartbeat_timer(self) -> None:
        """Start the heartbeat timer with proper synchronization."""
        with self._timer_lock:
            # Stop existing timer if running
            self._stop_heartbeat_timer()
            
            # Get interval from configuration
            heartbeat_config = self._get_heartbeat_config()
            interval_secs = heartbeat_config.get('intervalSecs', 30) if heartbeat_config else 30
            
            # Create and start new timer
            self._heartbeat_timer = threading.Timer(interval_secs, self._heartbeat_callback)
            self._heartbeat_timer.daemon = True
            self._heartbeat_timer.start()
            self._running = True
            
            logger.debug(f"Heartbeat timer started with {interval_secs}s interval")
            
    def _stop_heartbeat_timer(self) -> None:
        """Stop the heartbeat timer with proper cleanup."""
        if self._heartbeat_timer is not None:
            self._heartbeat_timer.cancel()
            self._heartbeat_timer = None
            
        self._running = False
        logger.debug("Heartbeat timer stopped")
        
    def _heartbeat_callback(self) -> None:
        """Heartbeat timer callback that publishes heartbeat data."""
        try:
            if not self._running:
                return
                
            # Publish heartbeat
            self._publish_heartbeat()
            
            # Schedule next heartbeat
            if self._running:
                self._start_heartbeat_timer()
                
        except Exception as e:
            logger.error(f"Error in heartbeat callback: {e}")
            # Try to restart timer on error
            if self._running:
                try:
                    self._start_heartbeat_timer()
                except Exception as restart_error:
                    logger.error(f"Failed to restart heartbeat timer: {restart_error}")
                    
    def _publish_heartbeat(self) -> None:
        """Publish heartbeat data to configured targets."""
        try:
            if self._heartbeat_monitor is None:
                logger.warning("Heartbeat monitor not initialized")
                return
                
            # Get system stats
            stats = self._heartbeat_monitor.get_stats()
            
            # Get heartbeat configuration
            heartbeat_config = self._get_heartbeat_config()
            if heartbeat_config is None:
                return
                
            targets = heartbeat_config.get('targets', [])
            
            # Publish to each configured target
            for target in targets:
                target_type = target.get('type', 'messaging')
                
                if target_type == 'messaging':
                    self._publish_to_messaging(stats, target)
                elif target_type == 'metric':
                    self._publish_to_metrics(stats)
                else:
                    logger.warning(f"Unknown heartbeat target type: {target_type}")
                    
        except Exception as e:
            logger.error(f"Failed to publish heartbeat: {e}")
            
    def _publish_to_messaging(self, stats: Dict[str, Any], target_config: Dict[str, Any]) -> None:
        """
        Publish heartbeat data to messaging target.
        
        Args:
            stats: System statistics data
            target_config: Target configuration
        """
        try:
            if self._messaging_service is None:
                logger.debug("No messaging service available for heartbeat")
                return
                
            # Import here to avoid circular imports
            from ggcommons.messaging.message_builder import MessageBuilder
            
            # Get topic and destination from config
            config = target_config.get('config', {})
            topic = config.get('topic', 'ggcommons/{ThingName}/{ComponentName}/heartbeat')
            destination = config.get('destination', 'ipc')
            
            # Resolve template variables
            resolved_topic = self._config_service.resolve_template(topic)
            
            # Build heartbeat message
            message = MessageBuilder.create(self.MESSAGE_NAME, self.MESSAGE_VERSION) \\\n                .with_payload(stats) \\\n                .with_config(self._config_service.config_manager) \\\n                .build()
            
            # Publish based on destination
            if destination.lower() == 'ipc':
                self._messaging_service.publish(resolved_topic, message)
            elif destination.lower() == 'iot_core':
                from awsiot.greengrasscoreipc.model import QOS
                self._messaging_service.publish_to_iot_core(resolved_topic, message, QOS.AT_LEAST_ONCE)
            else:
                logger.warning(f"Unknown messaging destination: {destination}")
                
            logger.debug(f"Published heartbeat to {destination} topic: {resolved_topic}")
            
        except Exception as e:
            logger.error(f"Failed to publish heartbeat to messaging: {e}")
            
    def _publish_to_metrics(self, stats: Dict[str, Any]) -> None:
        """
        Publish heartbeat data to metrics target.
        
        Args:
            stats: System statistics data
        """
        try:
            if self._metric_service is None:
                logger.debug("No metric service available for heartbeat")
                return
                
            # Flatten stats for metric emission
            measure_values = {}
            for category, values in stats.items():
                if isinstance(values, dict):
                    for measure_name, measure_value in values.items():
                        try:
                            measure_values[measure_name] = float(measure_value)
                        except (ValueError, TypeError):
                            logger.warning(f"Invalid metric value for {measure_name}: {measure_value}")
                            
            if measure_values:
                self._metric_service.emit_metric_now("heartbeat", measure_values)
                logger.debug(f"Published heartbeat metrics: {list(measure_values.keys())}")
                
        except Exception as e:
            logger.error(f"Failed to publish heartbeat to metrics: {e}")
            
    def on_configuration_change(self, configuration: Any) -> bool:
        """
        Handle configuration changes by reinitializing heartbeat.
        
        Args:
            configuration: New configuration
            
        Returns:
            True if configuration change was handled successfully
        """
        try:
            logger.info("Configuration changed, reinitializing heartbeat")
            
            with self._timer_lock:
                # Stop current heartbeat
                self._stop_heartbeat_timer()
                
                # Reinitialize with new configuration
                self._initialize_heartbeat()
                
            logger.info("Heartbeat reinitialized successfully")
            return True
            
        except Exception as e:
            logger.error(f"Failed to handle heartbeat configuration change: {e}")
            return False
            
    def start(self) -> None:
        """Start the heartbeat system."""
        try:
            with self._timer_lock:
                if not self._running:
                    self._start_heartbeat_timer()
                    logger.info("Heartbeat started")
                    
        except Exception as e:
            logger.error(f"Failed to start heartbeat: {e}")
            raise
            
    def stop(self) -> None:
        """Stop the heartbeat system and cleanup resources."""
        try:
            with self._timer_lock:
                self._stop_heartbeat_timer()
                
            # Remove configuration listener
            if self._config_service:
                self._config_service.remove_config_change_listener(self)
                
            logger.info("Heartbeat stopped and cleaned up")
            
        except Exception as e:
            logger.error(f"Error stopping heartbeat: {e}")
            
    def is_running(self) -> bool:
        """
        Check if heartbeat is currently running.
        
        Returns:
            True if heartbeat is running
        """
        return self._running
        
    def get_last_heartbeat_time(self) -> Optional[float]:
        """
        Get the timestamp of the last heartbeat.
        
        Returns:
            Timestamp of last heartbeat or None if not available
        """
        # Could implement heartbeat timestamp tracking here
        return None