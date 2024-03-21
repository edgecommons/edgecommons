import boto3
import time
from threading import Thread, Event
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget


class CloudWatch(MetricTarget):

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self._cloudwatch_client = boto3.client('cloudwatch', region_name='us-east-1')
        self._pending_metrics = {}
        self._interval_secs = -1
        self._flush_event = None
        self._flush_thread = None
        self._terminate_thread = False
        self._start_periodic_flush()

    def _start_periodic_flush(self):
        self._pending_metrics = {}
        self._interval_secs = self.config_manager.get_metric_config().get_interval_secs()
        self._flush_event = Event()
        self._flush_thread = Thread(target=self._flush_metrics_periodically, daemon=True)
        self._flush_thread.start()

    def _flush_metrics_periodically(self):
        while not self._flush_event.wait(self._interval_secs):
            self._flush_metrics()
            if self._terminate_thread:
                break

    def _flush_metrics(self):
        for namespace, metrics in self._pending_metrics.items():
            if metrics:
                self._cloudwatch_client.put_metric_data(Namespace=namespace, MetricData=metrics)
                self._pending_metrics[namespace] = []  # Clear after sending
        self.logger.info("Flushed pending metrics to CloudWatch.")

    def emit_metric(self, metric, measure_values):
        namespace = metric.get_namespace() if metric.get_namespace is not None \
            else self.config_manager.get_metric_config().get_namespace()
        metric_data = self._prepare_metric_data(metric, measure_values)
        if namespace not in self._pending_metrics:
            self._pending_metrics[namespace] = []
        self._pending_metrics[namespace].extend(metric_data)

    def emit_metric_now(self, metric, measure_values):
        namespace = metric.get_namespace() if metric.get_namespace is not None \
            else self.config_manager.get_metric_config().get_namespace()
        metric_data = self._prepare_metric_data(metric, measure_values)
        self._cloudwatch_client.put_metric_data(Namespace=namespace, MetricData=metric_data)
        self.logger.info(f"Metric {metric.name} sent to CloudWatch immediately.")

    def _prepare_metric_data(self, metric, measure_values):
        metric_data = []
        for measure_name, value in measure_values.items():
            data_point = {
                'MetricName': measure_name,
                'Dimensions': metric.dimensions_as_collection(),
                'Timestamp': time.time(),
                'Value': value,
                'Unit': metric.get_measure(measure_name).get_unit(),
                'StorageResolution': metric.get_measure(measure_name).get_storage_resolution()
            }
            metric_data.append(data_point)
        return metric_data

    def on_configuration_change(self, configuration) -> bool:
        self.logger.info("Configuration changed. Reconfiguring CloudWatch batch interval")
        self._terminate_thread = True
        self._flush_thread.join()
        self._terminate_thread = False
        self._flush_metrics()
        self._start_periodic_flush()
        return True
