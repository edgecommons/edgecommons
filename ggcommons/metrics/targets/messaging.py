from awsiot.greengrasscoreipc.model import QOS
from ggcommons.messaging.message import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.emf_helper import build_metric_data_emf
from ggcommons.metrics.targets.metric_target import MetricTarget


class Messaging(MetricTarget):

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.config_manager = config_manager
        self.topic = config_manager.resolve_template(config_manager.get_metric_config().get_topic())
        self.send_to_ipc = config_manager.get_metric_config().get_destination().lower() == "ipc"

    def emit_metric_now(self, metric, measure_values):
        metric_data = build_metric_data_emf(self.metric_config, metric, measure_values, False)
        self.__publish_message(metric_data)

        if self.metric_config.get_large_fleet_workaround():
            metric_data = build_metric_data_emf(self.metric_config, metric, measure_values, True)
            self.__publish_message(metric_data)

        self.logger.debug(f"Metric '{metric.get_name()}' emitted")

    def emit_metric(self, metric, measure_values):
        self.emit_metric_now(metric, measure_values)

    def __publish_message(self, metric_dict: dict):
        message = MessageBuilder.build_from_config("Metric", "1.0", metric_dict, self.config_manager)
        if self.send_to_ipc:
            MessagingClient.publish(self.topic, message)
        else:
            MessagingClient.publish_to_iot_core(self.topic, message, QOS.AT_LEAST_ONCE)

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Configuration changed. Reconfiguring messaging topic and destination")
        self.topic = self.config_manager.resolve_template(self.config_manager.get_metric_config().get_topic())
        self.send_to_ipc = self.config_manager.get_metric_config().get_destination().lower() == "ipc"
        return True
