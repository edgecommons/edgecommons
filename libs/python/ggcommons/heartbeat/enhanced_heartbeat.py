"""
Enhanced heartbeat system with service injection and improved lifecycle management.

The component heartbeat (UNS-CANONICAL-DESIGN §4.3, D-U14/D-U20): each tick it
publishes a UNS ``state`` keepalive to ``ecv1/{device}/{component}/main/state`` —
header name ``state``, body ``{"status":"RUNNING","uptimeSecs":<n>}`` — through the
privileged ``MessagingClient._publish_reserved*`` seam (the ``state`` class is
reserved), and emits the enabled system measures (cpu/memory/disk/...) as a metric
named ``sys`` through the normal metric subsystem (D6 — the measures keep the metric
subsystem's full sink routing). On graceful shutdown (:meth:`EnhancedHeartbeat.stop`)
a best-effort ``{"status":"STOPPED"}`` state is published. ``heartbeat.destination``
(``local`` | ``iotcore``) selects the keepalive's transport only. Defaults: on / 5 s /
local (M11).
"""

import logging
import threading
import time
from typing import Optional, Dict, Any, TYPE_CHECKING
from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.heartbeat.heartbeat_monitor import HeartbeatMonitor
from ggcommons.uns import Uns, UnsClass

if TYPE_CHECKING:
    from ggcommons.config.manager.config_manager import ConfigManager

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

    #: The state keepalive's envelope header name (§4.3).
    STATE_MESSAGE_NAME = "state"
    STATE_MESSAGE_VERSION = "1.0"
    #: The metric the heartbeat measures are emitted as (§4.3, D-U20/D6).
    SYS_METRIC_NAME = "sys"

    def __init__(self, config_service: "ConfigManager"):
        """
        Initialize enhanced heartbeat with the configuration manager.

        Args:
            config_service: ConfigManager for accessing heartbeat config

        Raises:
            ValueError: If config_service is None
        """
        if config_service is None:
            raise ValueError("Configuration manager cannot be None")

        super().__init__()
        self._config_service = config_service
        # Messaging/metric handles (the MessagingClient / MetricEmitter classes,
        # whose operations are static); injected via the setters below.
        self._messaging_service = None
        self._metric_service = None

        # Single long-lived loop thread driven by an Event (replaces the previous
        # self-rescheduling Timer chain, which spun up a new OS thread every tick
        # and had a _running read/write race). stop() sets the event to interrupt
        # the wait immediately and join the thread.
        self._heartbeat_thread: Optional[threading.Thread] = None
        self._stop_event = threading.Event()
        self._heartbeat_monitor: Optional[HeartbeatMonitor] = None
        self._timer_lock = threading.RLock()
        self._running = False
        # Monotonic start reference for the keepalive's uptimeSecs.
        self._start_monotonic = time.monotonic()
        # Ensures the best-effort STOPPED state is published at most once.
        self._stopped_published = False
        # WARN-once flag for the no-resolved-identity (test/subclass bring-up) case.
        self._warned_no_identity = False

        # Register for configuration changes
        self._config_service.add_config_change_listener(self)

        # Initialize heartbeat
        self._initialize_heartbeat()
        
    def set_messaging_service(self, messaging_service) -> None:
        """
        Set the messaging handle (the MessagingClient class).

        Args:
            messaging_service: The MessagingClient class (static-method API)
        """
        self._messaging_service = messaging_service
        name = getattr(messaging_service, "__name__", type(messaging_service).__name__)
        logger.info(f"Messaging handle set on heartbeat: {name}")

    def set_metric_service(self, metric_service) -> None:
        """
        Set the metric handle (the MetricEmitter class).

        Args:
            metric_service: The MetricEmitter class (static-method API)
        """
        self._metric_service = metric_service
        name = getattr(metric_service, "__name__", type(metric_service).__name__)
        logger.info(f"Metric handle set on heartbeat: {name}")
        # The metric must be defined now that a metric service is available: the
        # definition attempted during __init__ was a no-op because the service had
        # not been injected yet, which previously left ticks emitting an undefined
        # "heartbeat" metric.
        self._define_heartbeat_metric()
        
    def _initialize_heartbeat(self) -> None:
        """(Re)initialize the heartbeat from the current configuration: stops any
        running loop and, when ``heartbeat.enabled`` (the default), starts the periodic
        tick at ``heartbeat.intervalSecs``."""
        try:
            # Get heartbeat configuration
            heartbeat_config = self._get_heartbeat_config()
            if heartbeat_config is None:
                logger.warning("No heartbeat configuration found, using defaults")
                return

            with self._timer_lock:
                if not heartbeat_config.is_enabled():
                    self._stop_heartbeat_timer()
                    logger.info("Heartbeat disabled by configuration (heartbeat.enabled=false)")
                    return

                # Create heartbeat monitor
                self._heartbeat_monitor = HeartbeatMonitor(self._config_service)

                # Define metrics
                self._define_heartbeat_metric()

                # Start heartbeat timer
                self._start_heartbeat_timer()

            interval = heartbeat_config.get_interval_secs() if heartbeat_config else 5
            logger.info(
                f"Enhanced heartbeat initialized with {interval}s interval"
                f" (state keepalive -> {heartbeat_config.get_destination()})"
            )
            logger.debug(f"Messaging service available: {self._messaging_service is not None}")
            logger.debug(f"Metric service available: {self._metric_service is not None}")

        except Exception as e:
            logger.error(f"Failed to initialize enhanced heartbeat: {e}")
            raise
            
    def _get_heartbeat_config(self):
        """
        Get heartbeat configuration from config service.
        
        Returns:
            HeartbeatConfiguration object or None
        """
        try:
            return self._config_service.get_heartbeat_config()
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
            interval_secs = heartbeat_config.get_interval_secs() if heartbeat_config else 5
            storage_resolution = 1 if interval_secs < 60 else 60
            
            # Use the configured metricEmission namespace (parity with Java/Rust/TS), falling
            # back to the historical default only if no metric config is available.
            try:
                namespace = self._config_service.get_metric_config().get_namespace()
            except Exception:
                namespace = "GGCommons/Heartbeat"

            # Build the `sys` metric — the heartbeat measures (§4.3, D-U20/D6).
            metric = MetricBuilder.create(self.SYS_METRIC_NAME) \
                .with_namespace(namespace) \
                .with_thing_name(self._config_service.get_thing_name()) \
                .with_component_name(self._config_service.get_component_name()) \
                .add_measure("disk_total", "Gigabytes", storage_resolution) \
                .add_measure("disk_used", "Gigabytes", storage_resolution) \
                .add_measure("disk_free", "Gigabytes", storage_resolution) \
                .add_measure("cpu_usage", "Percent", storage_resolution) \
                .add_measure("memory_usage", "Megabytes", storage_resolution) \
                .add_measure("threads", "Count", storage_resolution) \
                .add_measure("files", "Count", storage_resolution) \
                .add_measure("fds", "Count", storage_resolution) \
                .build()
                
            self._metric_service.define_metric(metric)
            logger.debug(f"Heartbeat '{self.SYS_METRIC_NAME}' metric defined successfully")

        except Exception as e:
            logger.error(f"Failed to define heartbeat metric: {e}")
            
    def _get_interval_secs(self) -> float:
        """Resolve the heartbeat interval (seconds), defaulting to 30."""
        heartbeat_config = self._get_heartbeat_config()
        return heartbeat_config.get_interval_secs() if heartbeat_config else 5

    def _start_heartbeat_timer(self) -> None:
        """Start the heartbeat loop thread with proper synchronization."""
        with self._timer_lock:
            # Stop any existing loop first.
            self._stop_heartbeat_timer()

            # Fresh stop signal for the new loop.
            self._stop_event = threading.Event()
            self._heartbeat_thread = threading.Thread(
                target=self._run_loop, name="heartbeat", daemon=True
            )
            self._heartbeat_thread.start()
            self._running = True

            logger.debug(f"Heartbeat loop started with {self._get_interval_secs()}s interval")

    def _stop_heartbeat_timer(self) -> None:
        """Signal the heartbeat loop to stop and join it."""
        self._stop_event.set()
        thread = self._heartbeat_thread
        # Never join from within the loop thread itself.
        if thread is not None and thread is not threading.current_thread():
            thread.join(timeout=5)
        self._heartbeat_thread = None
        self._running = False
        logger.debug("Heartbeat loop stopped")

    def _run_loop(self) -> None:
        """Single loop thread: wait one interval, publish, repeat until stopped.

        Re-reads the interval each iteration so a configuration change takes
        effect, and guards each tick so a throwing publish cannot kill the loop.
        """
        while not self._stop_event.is_set():
            # wait() returns True if the stop event is set during the wait.
            if self._stop_event.wait(self._get_interval_secs()):
                break
            try:
                self._publish_heartbeat()
            except Exception as e:
                logger.error(f"Error in heartbeat loop: {e}")

    def _publish_heartbeat(self) -> None:
        """One heartbeat tick (§4.3): the ``state`` keepalive
        (``{"status":"RUNNING","uptimeSecs":n}``) plus the measures as the ``sys``
        metric. Each half is best-effort — a failure in one must not suppress the
        other."""
        try:
            self._publish_state("RUNNING", include_uptime=True)
        except Exception as e:  # noqa: BLE001 - each half is best-effort
            logger.warning(f"Heartbeat state keepalive failed: {e}")
        try:
            self._emit_sys_metric()
        except Exception as e:  # noqa: BLE001 - each half is best-effort
            logger.warning(f"Heartbeat '{self.SYS_METRIC_NAME}' metric emit failed: {e}")

    def _publish_state(self, status: str, include_uptime: bool) -> None:
        """Publishes one ``state`` envelope to the component's UNS state topic through
        the privileged seam. No-op (WARN once) when the component identity is not
        resolved (mock/test bring-up — a real ``ConfigManager`` always resolves one).

        :param status: ``"RUNNING"`` or ``"STOPPED"``
        :param include_uptime: whether the body carries ``uptimeSecs`` (the RUNNING
            keepalive)
        """
        if self._messaging_service is None:
            logger.warning("No messaging service available for heartbeat - service not injected yet")
            return
        identity = self._config_service.get_component_identity()
        if identity is None:
            if not self._warned_no_identity:
                self._warned_no_identity = True
                logger.warning(
                    "No resolved component identity - the heartbeat state keepalive is disabled"
                )
            return

        # Import here to avoid circular imports
        from ggcommons.messaging.message_builder import MessageBuilder

        topic = Uns(identity, self._config_service.is_topic_include_root()).topic(UnsClass.STATE)

        body: Dict[str, Any] = {"status": status}
        if include_uptime:
            body["uptimeSecs"] = int(time.monotonic() - self._start_monotonic)
        message = MessageBuilder.create(self.STATE_MESSAGE_NAME, self.STATE_MESSAGE_VERSION) \
            .with_payload(body) \
            .with_config(self._config_service) \
            .build()

        # The state class is reserved (§4.1) - publish through the privileged seam
        # (§4.2). heartbeat.destination selects the keepalive's transport only.
        heartbeat_config = self._get_heartbeat_config()
        destination = heartbeat_config.get_destination() if heartbeat_config else "local"
        if destination and destination.lower() in ("iotcore", "iot_core"):
            from awsiot.greengrasscoreipc.model import QOS
            self._messaging_service._publish_reserved_to_iot_core(topic, message, QOS.AT_LEAST_ONCE)
        else:
            self._messaging_service._publish_reserved(topic, message)
        logger.debug(f"Published heartbeat state '{status}' on topic: {topic}")

    def _emit_sys_metric(self) -> None:
        """Emits the enabled measures as the ``sys`` metric through the normal metric
        subsystem (its configured target: messaging/cloudwatch/EMF/log/prometheus)."""
        if self._metric_service is None:
            logger.debug("No metric service available for heartbeat")
            return
        if self._heartbeat_monitor is None:
            logger.warning("Heartbeat monitor not initialized")
            return

        stats = self._heartbeat_monitor.get_stats()

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
            self._metric_service.emit_metric_now(self.SYS_METRIC_NAME, measure_values)
            logger.debug(f"Published heartbeat measures: {list(measure_values.keys())}")
            
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
        """Stop the heartbeat system and cleanup resources, publishing the best-effort
        ``{"status":"STOPPED"}`` state (§4.3/D-U14 — at most once; failures are
        swallowed, the shutdown must proceed)."""
        try:
            with self._timer_lock:
                was_running = self._running
                self._stop_heartbeat_timer()

            if was_running and not self._stopped_published:
                self._stopped_published = True
                try:
                    self._publish_state("STOPPED", include_uptime=False)
                except Exception as e:  # noqa: BLE001 - best-effort by design
                    logger.debug(f"Best-effort STOPPED state publish failed: {e}")

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