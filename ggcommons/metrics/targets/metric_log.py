import logging
import json
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.emf_helper import build_metric_data_emf
from ggcommons.metrics.targets.metric_target import MetricTarget


class MetricLog(MetricTarget):
    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self._configure_logger()

    def _configure_logger(self):
        self.metric_logger = logging.getLogger("metric_file")
        self.metric_logger.setLevel(logging.INFO)
        log_file_path_template = (
            self.config_manager.get_metric_config().get_log_file_name_template()
        )
        log_file_path = self.config_manager.resolve_template(log_file_path_template)
        handler = logging.FileHandler(log_file_path)
        formatter = logging.Formatter("%(message)s")  # EMF metrics are in JSON format
        handler.setFormatter(formatter)
        self.metric_logger.addHandler(handler)
        self.metric_logger.propagate = False

    def emit_metric_now(self, metric, measure_values):
        metric_data = build_metric_data_emf(
            self.metric_config, metric, measure_values, False
        )
        self.metric_logger.info(json.dumps(metric_data))
        if self.metric_config.get_large_fleet_workaround():
            metric_data = build_metric_data_emf(
                self.metric_config, metric, measure_values, True
            )
            self.metric_logger.info(json.dumps(metric_data))
        self.logger.debug(f"Metric '{metric.get_name()}' emitted")

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Configuration changed. Reconfiguring metric logger")
        self._configure_logger()
        return True
