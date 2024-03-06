import logging
import threading
import time
from abc import ABC

from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.heartbeat.heartbeat_monitor import HeartbeatMonitor
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.measure import Measure

logger = logging.getLogger("Heartbeat")


class Heartbeat(ConfigurationChangeListener, ABC):
    __MESSAGE_NAME = "heartbeat"
    __MESSAGE_VERSION = "1.0.0"

    def __init__(self, configuration_manager: ConfigManager):
        super().__init__()
        self._heartbeat_thread = None
        self._configuration_manager = configuration_manager
        self._configuration_manager.add_config_change_listener(self)
        # self.__compute_topic()
        self.keep_running = True
        self._define_metric(configuration_manager)
        self.__run_heartbeat()

    def _define_metric(self, configuration_manager: ConfigManager):
        storage_resolution = 1 if configuration_manager.get_heartbeat_config().get_interval_secs() < 60 else 60
        metric = Metric("heartbeat","ggcommons0305_1")
        metric.add_measure(Measure("disk_total", "Gigabytes", storage_resolution))
        metric.add_measure(Measure("disk_used", "Gigabytes", storage_resolution))
        metric.add_measure(Measure("disk_free", "Gigabytes", storage_resolution))
        metric.add_measure(Measure("cpu_usage", "Percent", storage_resolution))
        metric.add_measure(Measure("memory_usage", "Megabytes", storage_resolution))
        metric.add_measure(Measure("threads", "Count", storage_resolution))
        metric.add_measure(Measure("files", "Count", storage_resolution))
        metric.add_measure(Measure("fds", "Count", storage_resolution))
        MetricEmitter.define_metric(metric)

    @staticmethod
    def __publish_heartbeat(heartbeat_monitor: HeartbeatMonitor):
        data = heartbeat_monitor.get_stats()
        measure_values = {}
        if "disk" in data:
            for key, value in data["disk"].items():
                if isinstance(value, dict):  # Assuming the structure is similar to a JsonObject
                    for measure_name, measure_value in value.items():
                        measure_values[measure_name] = float(measure_value)
        for key, value in data.items():
            if isinstance(value, dict):  # Assuming the structure is similar to a JsonObject
                for measure_name, measure_value in value.items():
                    measure_values[measure_name] = float(measure_value)
        MetricEmitter.emit_metric_now("heartbeat", measure_values)

    """
    @staticmethod
    def __publish_heartbeat(
            heartbeat_monitor: HeartbeatMonitor,
            configuration_manager: ConfigManager,
    ):
        logger.debug("Publishing heartbeat...")
        data = heartbeat_monitor.get_stats()
        # thing_name = configuration_manager.get_thing_name()
        # component_name = configuration_manager.get_component_name()

        # Define dimensions for all metrics
        # dimensions = [{"name": "Thing", "value": thing_name}, {"name": "Component", "value": component_name}]

        # Handle disk usage metrics
        if "disk" in data:
            for metric_name, value in data["disk"].items():
                unit = "Gigabytes"  # since disk metrics are explicitly in Gigabytes
                # Formatting for consistency
                formatted_metric_name = "disk_" + metric_name.replace("(GB)", "").strip().replace(" ",
                                                                                                  "_").lower()
                MetricEmitter.define_metric(formatted_metric_name, unit)
                MetricEmitter.emit_metric_now(formatted_metric_name, value)

        # Handle other metrics
        for metric_name, value in data.items():
            if metric_name in ["cpu", "memory", "threads", "files", "fds"]:
                if metric_name == "cpu":
                    MetricEmitter.define_metric("cpu_usage", "Percent")
                    MetricEmitter.emit_metric_now("cpu_usage", value["cpu_usage(%)"])
                elif metric_name == "memory":
                    MetricEmitter.define_metric("memory_usage", "Megabytes")
                    MetricEmitter.emit_metric_now("memory_usage", value["memory_usage(MB)"])
                else:
                    unit = "Count"
                    formatted_metric_name = metric_name + "_count"
                    MetricEmitter.define_metric(formatted_metric_name, unit)
                    MetricEmitter.emit_metric_now(formatted_metric_name, value[metric_name])
        
        else:
            message = MessageBuilder.build_from_config(
                name=Heartbeat.__MESSAGE_NAME,
                version=Heartbeat.__MESSAGE_VERSION,
                payload=data,
                config_manager=configuration_manager,
            )
            MessagingClient.publish(topic, message)
        

    def __compute_topic(self):
        self._topic = self._configuration_manager.resolve_template(
            self._configuration_manager.get_heartbeat_config().get_topic()
        )
    """

    @staticmethod
    def __heartbeat_loop(heartbeater, configuration_manager: ConfigManager):
        heartbeat_monitor = HeartbeatMonitor(
            configuration_manager
        )
        logger.debug(f"Starting heartbeat")
        try:
            while heartbeater.keep_running:
                heartbeater.__publish_heartbeat(
                    heartbeat_monitor
                )
                time.sleep(
                    configuration_manager.get_heartbeat_config().get_interval_secs()
                )
        except KeyboardInterrupt:
            logger.error("Publishing loop stopped.")
        except Exception as exc:
            logger.exception(f"Error while publishing heartbeat message: {exc}")

    def __run_heartbeat(self):
        try:
            thread_name = (
                f"{self._configuration_manager.get_component_name()}-heartbeat"
            )
            self._heartbeat_thread = threading.Thread(
                target=Heartbeat.__heartbeat_loop,
                args=(
                    self,
                    self._configuration_manager,
                ),
                name=thread_name,
            )
            self._heartbeat_thread.daemon = True
            self._heartbeat_thread.start()
        except Exception as exc:
            logger.exception("Error while starting heartbeat thread" + str(exc))

    def on_configuration_change(self, configuration) -> bool:
        logger.debug("Configuration changed, restarting heartbeat")
        self.keep_running = False
        self._heartbeat_thread.join()
        self.keep_running = True
        self.__run_heartbeat()
        logger.debug("Heartbeat restarted")
        return True
