import boto3
import logging
import time
from threading import Thread, Event
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget


class CloudWatch(MetricTarget):
    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.cloudwatch_client = boto3.client('cloudwatch', region_name='us-east-1')
        self.pending_metrics = {}
        self.interval_secs = config_manager.get_metric_config().get_interval_secs()
        self.flush_event = Event()
        self.start_periodic_flush()

    def start_periodic_flush(self):
        Thread(target=self.flush_metrics_periodically, daemon=True).start()

    def flush_metrics_periodically(self):
        while not self.flush_event.wait(self.interval_secs):
            self.flush_metrics()

    def flush_metrics(self):
        for namespace, metrics in self.pending_metrics.items():
            if metrics:
                self.cloudwatch_client.put_metric_data(Namespace=namespace, MetricData=metrics)
                self.pending_metrics[namespace] = []  # Clear after sending
        self.logger.info("Flushed pending metrics to CloudWatch.")

    def emit_metric(self, metric, measure_values):
        namespace = metric.get_namespace()
        metric_data = self.prepare_metric_data(metric, measure_values)
        if namespace not in self.pending_metrics:
            self.pending_metrics[namespace] = []
        self.pending_metrics[namespace].extend(metric_data)

    def emit_metric_now(self, metric, measure_values):
        namespace = metric.get_namespace()
        metric_data = self.prepare_metric_data(metric, measure_values)
        self.cloudwatch_client.put_metric_data(Namespace=namespace, MetricData=metric_data)
        self.logger.info(f"Metric {metric.name} sent to CloudWatch immediately.")

    def prepare_metric_data(self, metric, measure_values):
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
