import logging
import threading
import time
from abc import ABC

from awsiot.greengrasscoreipc.model import QOS

from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.heartbeat.heartbeat_monitor import HeartbeatMonitor
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.measure import Measure
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("Heartbeat")


class Heartbeat(ConfigurationChangeListener, ABC):
    __MESSAGE_NAME = "heartbeat"
    __MESSAGE_VERSION = "1.0.0"

    def __init__(self, config_service: "ConfigManager"):
        super().__init__()
        logger.info(f"Initializing Heartbeat system for component: {config_service.get_component_name()}")
        
        self._heartbeat_thread = None
        self._config_service = config_service
        self._config_service.add_config_change_listener(self)
        self._config = self._config_service.get_heartbeat_config()
        self.keep_running = True
        
        logger.info(f"Heartbeat configured - interval: {self._config.get_interval_secs()}s, targets: {len(self._config.get_targets())}")
        logger.debug(f"Heartbeat targets: {[target['type'] for target in self._config.get_targets()]}")
        
        self._define_metric(config_service)
        self.__run_heartbeat()
        
        logger.info("Heartbeat system initialization completed")

    def _define_metric(self, config_service: "ConfigManager"):
        logger.info("Defining heartbeat metrics")
        
        storage_resolution = (
            1
            if config_service.get_heartbeat_config().get_interval_secs() < 60
            else 60
        )
        
        logger.debug(f"Heartbeat metric storage resolution: {storage_resolution}s")
        
        from ggcommons.metrics.metric_builder import MetricBuilder
        
        measures = [
            ("disk_total", "Gigabytes"), ("disk_used", "Gigabytes"), ("disk_free", "Gigabytes"),
            ("cpu_usage", "Percent"), ("memory_usage", "Megabytes"), ("threads", "Count"),
            ("files", "Count"), ("fds", "Count")
        ]
        
        metric_builder = MetricBuilder.create("heartbeat") \
            .with_namespace(config_service.get_metric_config().get_namespace())
        
        for measure_name, unit in measures:
            metric_builder.add_measure(measure_name, unit, storage_resolution)
        
        metric = metric_builder.build()
        
        MetricEmitter.define_metric(metric)
        logger.info(f"Heartbeat metric defined with {len(measures)} measures")

    def __publish_heartbeat(self, heartbeat_monitor: HeartbeatMonitor):
        data = heartbeat_monitor.get_stats()
        logger.debug(f"Publishing heartbeat data with {len(data)} metrics")
        
        for target in self._config.get_targets():
            target_type = target["type"]
            logger.debug(f"Processing heartbeat target: {target_type}")
            
            if target_type == "messaging":
                message = MessageBuilder.create(
                    Heartbeat.__MESSAGE_NAME,
                    Heartbeat.__MESSAGE_VERSION
                ).with_payload(data).with_config(self._config_service).build()
                
                topic = self._config.DEFAULT_HEARTBEAT_MESSAGING_TOPIC
                destination = self._config.DEFAULT_HEARTBEAT_MESSAGING_DESTINATION
                
                if "config" in target:
                    if "topic" in target["config"]:
                        topic = self._config_service.resolve_template(
                            target["config"]["topic"]
                        )
                    if "destination" in target["config"]:
                        destination = target["config"]["destination"]
                
                logger.debug(f"Publishing heartbeat to {destination} destination on topic: {topic}")
                
                if destination.lower() == "ipc":
                    MessagingClient.publish(topic, message)
                else:
                    MessagingClient.publish_to_iot_core(
                        topic, message, QOS.AT_LEAST_ONCE
                    )
                    
            elif target_type == "metric":
                measure_values = {}
                for key, value in data.items():
                    if isinstance(value, dict):
                        for measure_name, measure_value in value.items():
                            measure_values[measure_name] = float(measure_value)
                
                logger.debug(f"Emitting heartbeat metrics with {len(measure_values)} measures")
                MetricEmitter.emit_metric_now("heartbeat", measure_values)

    def __heartbeat_loop(self):
        heartbeat_monitor = HeartbeatMonitor(self._config_service)
        interval = self._config_service.get_heartbeat_config().get_interval_secs()
        
        logger.info(f"Starting heartbeat loop with {interval}s interval")
        
        try:
            while self.keep_running:
                logger.debug("Heartbeat cycle starting")
                self.__publish_heartbeat(heartbeat_monitor)
                logger.debug(f"Heartbeat cycle completed, sleeping for {interval}s")
                time.sleep(interval)
            logger.info("Heartbeat loop stopped intentionally")
        except KeyboardInterrupt:
            logger.error("Heartbeat loop interrupted by user")
        except Exception as exc:
            logger.exception(f"Error in heartbeat loop: {exc}")

    def __run_heartbeat(self):
        try:
            thread_name = f"{self._config_service.get_component_name()}-heartbeat"
            logger.info(f"Starting heartbeat thread: {thread_name}")
            
            self._heartbeat_thread = threading.Thread(
                target=self.__heartbeat_loop,
                name=thread_name,
            )
            self._heartbeat_thread.daemon = True
            self._heartbeat_thread.start()
            
            logger.info(f"Heartbeat thread started successfully: {thread_name}")
            
        except Exception as exc:
            logger.error(f"Failed to start heartbeat thread: {exc}")
            raise

    def on_configuration_change(self, configuration) -> bool:
        logger.info("Heartbeat configuration changed, restarting heartbeat system")
        
        # Stop current heartbeat
        logger.debug("Stopping current heartbeat thread")
        self.keep_running = False
        self._heartbeat_thread.join()
        
        # Restart with new configuration
        logger.debug("Reloading heartbeat configuration")
        self.keep_running = True
        self._config = self._config_service.get_heartbeat_config()
        
        logger.info(f"Heartbeat reconfigured - new interval: {self._config.get_interval_secs()}s")
        
        self._define_metric(self._config_service)
        self.__run_heartbeat()
        
        logger.info("Heartbeat system restart completed")
        return True

    def stop(self):
        logger.info("Stopping heartbeat system")
        self.keep_running = False
        if self._heartbeat_thread:
            self._heartbeat_thread.join()
        logger.info("Heartbeat system stopped")

    def start(self):
        logger.info("Starting heartbeat system")
        self.keep_running = True
        self.__run_heartbeat()
