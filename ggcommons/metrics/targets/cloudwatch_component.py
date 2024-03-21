import time
from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget


class CloudWatchComponent(MetricTarget):

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.topic = config_manager.resolve_template(config_manager.get_metric_config().get_topic())

    def emit_metric_now(self, metric, measure_values):
        for measure_name, measure_value in measure_values.items():
            metric_data = self.build_metric_data(metric, measure_name, measure_value)
            MessagingClient.publish_raw(self.topic, metric_data)
            self.logger.info(f"Metric {metric.get_name()} emitted")

    def emit_metric(self, metric, measure_values):
        self.emit_metric_now(metric, measure_values)

    def build_metric_data(self, metric, measure_name, measure_value):
        metric_data = {
            "metricName": measure_name,
            "value": measure_value,
            "unit": metric.get_measure(measure_name).get_unit(),
            "dimensions": metric.dimensions_as_json(include_core_name=False)
        }
        namespace = metric.get_namespace() if metric.get_namespace is not None \
            else self.config_manager.get_metric_config().get_namespace()
        data = {
           "request": {
               "namespace": namespace,
               "timestamp": int(time.time()),
               "metricData": metric_data
            }
        }
        return data

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Configuration changed. Reconfiguring cloudwatch component topic")
        self.topic = self.config_manager.resolve_template(self.config_manager.get_metric_config().get_topic())
        return True
