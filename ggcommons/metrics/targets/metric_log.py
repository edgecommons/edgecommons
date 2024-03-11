import logging
import time
import json
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget


class MetricLog(MetricTarget):

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self._configure_logger()

    def _configure_logger(self):
        self.metric_logger = logging.getLogger("metric_file")
        self.metric_logger.setLevel(logging.INFO)
        log_file_path_template = self.config_manager.get_metric_config().get_log_file_name_template()
        log_file_path = self.config_manager.resolve_template(log_file_path_template)
        handler = logging.FileHandler(log_file_path)
        formatter = logging.Formatter('%(message)s')  # EMF metrics are in JSON format
        handler.setFormatter(formatter)
        self.metric_logger.addHandler(handler)
        self.metric_logger.propagate = False

    def emit_metric(self, metric, measure_values):
        self.emit_metric_now(metric, measure_values)

    def emit_metric_now(self, metric, measure_values):
        metric_data = self.build_metric_data(metric, measure_values)
        self.metric_logger.info(json.dumps(metric_data))
        self.logger.info(f"Metric emitted for {metric.get_name()} emitted")

    def build_metric_data(self, metric, measure_values):
        emf_object = {}

        aws_object = {
            "Timestamp": int(time.time()),
            "CloudWatchMetrics": [self.get_metrics_metadata(metric)]
        }

        emf_object["_aws"] = aws_object
        for key, value in metric.get_dimensions().items():
            emf_object[key] = value
        for key, value in measure_values.items():
            emf_object[key] = value

        return emf_object

    def get_metrics_metadata(self, metric):
        cw_metrics_array_entry = {
            "Namespace": self.config_manager.get_metric_config().get_namespace(),
            "Dimensions": [[dimension for dimension in metric.get_dimensions().keys()]],
            "Metrics": [{
                "Name": measure.get_name(),
                "Unit": measure.get_unit(),
                "StorageResolution": measure.get_storage_resolution()
            } for measure in metric.get_measures().values()]
        }
        return cw_metrics_array_entry

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Configuration changed. Reconfiguring metric logger")
        self._configure_logger()
        return True
