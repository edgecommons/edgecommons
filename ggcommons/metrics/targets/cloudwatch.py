import boto3
import time
from threading import Thread, Event
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget


class CloudWatch(MetricTarget):
    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.logger.info("Initializing CloudWatch metric target")
        
        self._cloudwatch_client = boto3.client("cloudwatch", region_name="us-east-1")
        self._pending_metrics = {}
        self._interval_secs = -1
        self._flush_event = None
        self._flush_thread = None
        self._terminate_thread = False
        
        self.logger.info(f"CloudWatch client initialized for region: us-east-1")
        self._start_periodic_flush()

    def _start_periodic_flush(self):
        self._pending_metrics = {}
        self._interval_secs = (
            self.config_manager.get_metric_config().get_interval_secs()
        )
        
        self.logger.info(f"Starting CloudWatch periodic flush with {self._interval_secs}s interval")
        
        self._flush_event = Event()
        self._flush_thread = Thread(
            target=self._flush_metrics_periodically, daemon=True
        )
        self._flush_thread.start()
        
        self.logger.debug("CloudWatch flush thread started")

    def _flush_metrics_periodically(self):
        while not self._flush_event.wait(self._interval_secs):
            self._flush_metrics()
            if self._terminate_thread:
                break

    def _flush_metrics(self):
        total_metrics = sum(len(metrics) for metrics in self._pending_metrics.values())
        if total_metrics == 0:
            self.logger.debug("No pending metrics to flush to CloudWatch")
            return
            
        self.logger.debug(f"Flushing {total_metrics} metrics across {len(self._pending_metrics)} namespaces to CloudWatch")
        
        for namespace, metrics in self._pending_metrics.items():
            if metrics:
                self.logger.debug(f"Sending {len(metrics)} metrics to CloudWatch namespace: {namespace}")
                self._cloudwatch_client.put_metric_data(
                    Namespace=namespace, MetricData=metrics
                )
                self._pending_metrics[namespace] = []  # Clear after sending
                
        self.logger.debug(f"Successfully flushed {total_metrics} metrics to CloudWatch")

    def emit_metric(self, metric, measure_values):
        namespace = (
            metric.get_namespace()
            if metric.get_namespace is not None
            else self.config_manager.get_metric_config().get_namespace()
        )
        
        self.logger.debug(f"Queuing metric '{metric.get_name()}' for CloudWatch namespace: {namespace} with {len(measure_values)} measures")
        
        metric_data = self._prepare_metric_data(metric, measure_values)
        if namespace not in self._pending_metrics:
            self._pending_metrics[namespace] = []
        self._pending_metrics[namespace].extend(metric_data)
        
        total_pending = len(self._pending_metrics[namespace])
        self.logger.debug(f"Metric '{metric.get_name()}' queued for CloudWatch - {total_pending} metrics pending in namespace {namespace}")

    def emit_metric_now(self, metric, measure_values):
        namespace = (
            metric.get_namespace()
            if metric.get_namespace() is not None
            else self.config_manager.get_metric_config().get_namespace()
        )
        
        self.logger.debug(f"Emitting metric '{metric.get_name()}' immediately to CloudWatch namespace: {namespace} with {len(measure_values)} measures")
        
        metric_data = self._prepare_metric_data(metric, measure_values)
        self._cloudwatch_client.put_metric_data(
            Namespace=namespace, MetricData=metric_data
        )
        
        self.logger.debug(f"Metric '{metric.get_name()}' sent immediately to CloudWatch - {len(metric_data)} data points")

    def _prepare_metric_data(self, metric, measure_values):
        metric_data = []
        for measure_name, value in measure_values.items():
            data_point = {
                "MetricName": measure_name,
                "Dimensions": metric.dimensions_as_collection(False),
                "Timestamp": time.time(),
                "Value": value,
                "Unit": metric.get_measure(measure_name).get_unit(),
                "StorageResolution": metric.get_measure(
                    measure_name
                ).get_storage_resolution(),
            }
            metric_data.append(data_point)
            if self.metric_config.get_large_fleet_workaround():
                data_point = {
                    "MetricName": measure_name,
                    "Dimensions": metric.dimensions_as_collection(True),
                    "Timestamp": time.time(),
                    "Value": value,
                    "Unit": metric.get_measure(measure_name).get_unit(),
                    "StorageResolution": metric.get_measure(
                        measure_name
                    ).get_storage_resolution(),
                }
                metric_data.append(data_point)
        return metric_data

    def on_configuration_change(self, configuration) -> bool:
        old_interval = self._interval_secs
        new_interval = self.config_manager.get_metric_config().get_interval_secs()
        
        self.logger.info(f"CloudWatch configuration changed - interval: {old_interval}s -> {new_interval}s")
        
        # Stop current flush thread
        self.logger.debug("Stopping current CloudWatch flush thread")
        self._terminate_thread = True
        self._flush_thread.join()
        self._terminate_thread = False
        
        # Flush any pending metrics before reconfiguring
        self.logger.debug("Flushing pending metrics before reconfiguration")
        self._flush_metrics()
        
        # Start new flush thread with updated configuration
        self._start_periodic_flush()
        
        self.logger.info("CloudWatch target reconfiguration completed")
        return True
