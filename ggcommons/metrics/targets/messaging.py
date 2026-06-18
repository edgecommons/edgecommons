from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.emf_helper import build_metric_data_emf
from ggcommons.metrics.targets.metric_target import MetricTarget


def _is_local_destination(destination: str) -> bool:
    """True for the local/IPC transport, False for IoT Core.

    Canonical values are "ipc" (local/IPC) and "iot_core" (IoT Core); the legacy
    "local"/"iotcore" spellings are also accepted. Mirrors the heartbeat target,
    the Java/Rust metric targets, and the config schema.
    """
    return destination.lower() in ("ipc", "local")


class Messaging(MetricTarget):
    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.topic = config_manager.resolve_template(
            config_manager.get_metric_config().get_topic()
        )
        self.send_to_local = _is_local_destination(
            config_manager.get_metric_config().get_destination()
        )

    def emit_metric_now(self, metric, measure_values):
        metric_name = metric.get_name()
        self.logger.debug(f"Emitting metric '{metric_name}' to messaging target with {len(measure_values)} measures")
        
        metric_data = build_metric_data_emf(
            self.metric_config, metric, measure_values, False
        )
        self.__publish_message(metric_data)
        
        if self.metric_config.get_large_fleet_workaround():
            self.logger.debug(f"Emitting large fleet workaround metric for '{metric_name}'")
            metric_data = build_metric_data_emf(
                self.metric_config, metric, measure_values, True
            )
            self.__publish_message(metric_data)

        self.logger.debug(f"Metric '{metric_name}' emission completed")

    def __publish_message(self, metric_dict: dict):
        destination = "local" if self.send_to_local else "IoT Core"
        self.logger.debug(f"Publishing metric message to {destination} on topic: {self.topic}")
        
        message = MessageBuilder.create("Metric", "1.0") \
            .with_payload(metric_dict) \
            .with_config(self.config_manager) \
            .build()
            
        if self.send_to_local:
            MessagingClient.publish(self.topic, message)
        else:
            MessagingClient.publish_to_iot_core(self.topic, message, QOS.AT_LEAST_ONCE)

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Metric messaging configuration changed, reconfiguring target")
        
        old_topic = self.topic
        old_destination = "local" if self.send_to_local else "IoT Core"
        
        self.topic = self.config_manager.resolve_template(
            self.config_manager.get_metric_config().get_topic()
        )
        self.send_to_local = _is_local_destination(
            self.config_manager.get_metric_config().get_destination()
        )

        new_destination = "local" if self.send_to_local else "IoT Core"
        
        self.logger.info(f"Metric messaging reconfigured - topic: {old_topic} -> {self.topic}, destination: {old_destination} -> {new_destination}")
        return True
