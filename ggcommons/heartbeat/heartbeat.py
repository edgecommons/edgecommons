import logging
import threading
import time
from abc import ABC

from awsiot.greengrasscoreipc.model import QOS

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
        self._config = self._configuration_manager.get_heartbeat_config()
        # self.__compute_topic()
        self.keep_running = True
        self._define_metric(configuration_manager)
        self.__run_heartbeat()

    def _define_metric(self, configuration_manager: ConfigManager):
        storage_resolution = 1 if configuration_manager.get_heartbeat_config().get_interval_secs() < 60 else 60
        metric = Metric("heartbeat", configuration_manager.get_metric_config().get_namespace())
        metric.add_measure(Measure("disk_total", "Gigabytes", storage_resolution))
        metric.add_measure(Measure("disk_used", "Gigabytes", storage_resolution))
        metric.add_measure(Measure("disk_free", "Gigabytes", storage_resolution))
        metric.add_measure(Measure("cpu_usage", "Percent", storage_resolution))
        metric.add_measure(Measure("memory_usage", "Megabytes", storage_resolution))
        metric.add_measure(Measure("threads", "Count", storage_resolution))
        metric.add_measure(Measure("files", "Count", storage_resolution))
        metric.add_measure(Measure("fds", "Count", storage_resolution))
        MetricEmitter.define_metric(metric)

    def __publish_heartbeat(self, heartbeat_monitor: HeartbeatMonitor):
        data = heartbeat_monitor.get_stats()
        for target in self._config.get_targets():
            if target["type"] == "messaging":
                message = MessageBuilder.build_from_config(
                    Heartbeat.__MESSAGE_NAME,
                    Heartbeat.__MESSAGE_VERSION,
                    data,
                    self._configuration_manager,
                )
                topic = self._config.DEFAULT_HEARTBEAT_MESSAGING_TOPIC
                destination = self._config.DEFAULT_HEARTBEAT_MESSAGING_DESTINATION
                if "config" in target:
                    if "topic" in target["config"]:
                        topic = self._configuration_manager.resolve_template(target["config"]["topic"])
                    if "destination" in target["config"]:
                        destination = target["config"]["destination"]
                if destination.lower() == "ipc":
                    MessagingClient.publish(topic, message)
                else:
                    MessagingClient.publish_to_iot_core(topic, message, QOS.AT_LEAST_ONCE)
            elif target["type"] == "metric":
                measure_values = {}
                for key, value in data.items():
                    if isinstance(value, dict):  # Assuming the structure is similar to a JsonObject
                        for measure_name, measure_value in value.items():
                            measure_values[measure_name] = float(measure_value)
                MetricEmitter.emit_metric_now("heartbeat", measure_values)

    def __heartbeat_loop(self):
        heartbeat_monitor = HeartbeatMonitor(self._configuration_manager)
        logger.debug(f"Starting heartbeat")
        try:
            while self.keep_running:
                self.__publish_heartbeat(
                    heartbeat_monitor
                )
                time.sleep(
                    self._configuration_manager.get_heartbeat_config().get_interval_secs()
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
                target=self.__heartbeat_loop,
                name=thread_name,
            )
            self._heartbeat_thread.daemon = True
            self._heartbeat_thread.start()
        except Exception as exc:
            logger.exception("Error while starting heartbeat thread" + str(exc))

    def on_configuration_change(self, configuration) -> bool:
        logger.info("Configuration changed, restarting heartbeat")
        self.keep_running = False
        self._heartbeat_thread.join()
        self.keep_running = True
        self._config = self._configuration_manager.get_heartbeat_config()
        self._define_metric(self._configuration_manager)
        self.__run_heartbeat()
        logger.info("Heartbeat restarted")
        return True
