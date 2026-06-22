import boto3
import time
from threading import Thread, Event, Lock
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.metric_target import MetricTarget


class CloudWatch(MetricTarget):
    # CloudWatch PutMetricData accepts at most 1000 metric datums per request.
    MAX_DATUMS_PER_REQUEST = 1000

    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        self.logger.info("Initializing CloudWatch metric target")
        
        # Resolve the region from the standard AWS chain (env/config/IMDS/TES) like the
        # Java/Rust/TS targets do, instead of hardcoding us-east-1. Fall back to us-east-1
        # only if no region is resolvable, preserving the previous behavior.
        try:
            self._cloudwatch_client = boto3.client("cloudwatch")
        except Exception:
            self._cloudwatch_client = boto3.client("cloudwatch", region_name="us-east-1")
        self._pending_metrics = {}
        # Guards _pending_metrics against the flush thread's read-modify-write
        # racing with concurrent emit_metric() appends.
        self._pending_lock = Lock()
        self._interval_secs = -1
        self._flush_event = None
        self._flush_thread = None
        self._terminate_thread = False
        
        self.logger.info(
            f"CloudWatch client initialized for region: {self._cloudwatch_client.meta.region_name}"
        )
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

    def close(self) -> None:
        """Stop the periodic-flush thread and flush any remaining metrics."""
        self._terminate_thread = True
        if self._flush_event is not None:
            self._flush_event.set()  # wake the pending wait()
        if self._flush_thread is not None:
            self._flush_thread.join(timeout=5)
            self._flush_thread = None
        # Best-effort final flush of anything still pending.
        try:
            self._flush_metrics()
        except Exception as e:
            self.logger.warning(f"Error during final CloudWatch flush: {e}")

    def _flush_metrics_periodically(self):
        while not self._flush_event.wait(self._interval_secs):
            # Never let an unexpected error kill the flush thread.
            try:
                self._flush_metrics()
            except Exception as e:
                self.logger.error(f"Unexpected error during CloudWatch flush: {e}")
            if self._terminate_thread:
                break

    def _flush_metrics(self):
        # Atomically take everything pending and reset it, so metrics emitted
        # during the (network-bound) send below queue for the next flush rather
        # than being dropped by a concurrent read-modify-write.
        with self._pending_lock:
            snapshot = self._pending_metrics
            self._pending_metrics = {}

        total_metrics = sum(len(metrics) for metrics in snapshot.values())
        if total_metrics == 0:
            self.logger.debug("No pending metrics to flush to CloudWatch")
            return

        self.logger.debug(f"Flushing {total_metrics} metrics across {len(snapshot)} namespaces to CloudWatch")

        for namespace, metrics in snapshot.items():
            if not metrics:
                continue
            # Isolate each namespace so one failing PutMetricData does not drop the
            # others, and chunk into <=1000-datum batches (CloudWatch's per-request
            # limit) so large batches are not rejected wholesale. On failure, keep
            # only the not-yet-sent datums so the next flush retries without
            # re-sending datums already accepted.
            remaining = []
            failed = False
            for start in range(0, len(metrics), self.MAX_DATUMS_PER_REQUEST):
                batch = metrics[start:start + self.MAX_DATUMS_PER_REQUEST]
                if failed:
                    remaining.extend(batch)
                    continue
                try:
                    self.logger.debug(f"Sending {len(batch)} metrics to CloudWatch namespace: {namespace}")
                    self._cloudwatch_client.put_metric_data(
                        Namespace=namespace, MetricData=batch
                    )
                except Exception as e:
                    self.logger.error(
                        f"Error sending metrics to CloudWatch namespace '{namespace}': {e}. "
                        f"Will retry on next flush."
                    )
                    failed = True
                    remaining.extend(batch)
            # Write back the not-yet-sent datums for this namespace, ahead of
            # anything emitted concurrently during the flush.
            with self._pending_lock:
                self._pending_metrics[namespace] = (
                    remaining + self._pending_metrics.get(namespace, [])
                )

        self.logger.debug("CloudWatch flush cycle complete")

    def emit_metric(self, metric, measure_values):
        namespace = (
            metric.get_namespace()
            if metric.get_namespace() is not None
            else self.config_manager.get_metric_config().get_namespace()
        )

        self.logger.debug(f"Queuing metric '{metric.get_name()}' for CloudWatch namespace: {namespace} with {len(measure_values)} measures")

        metric_data = self._prepare_metric_data(metric, measure_values)
        with self._pending_lock:
            self._pending_metrics.setdefault(namespace, []).extend(metric_data)
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
            measure = metric.get_measure(measure_name)
            if measure is None:
                # Defensive: an emit naming a measure the metric never defined must not crash the
                # component — it only affects this one data point. Skip it with a warning.
                # (Previously this raised AttributeError on None.get_unit(), which propagated out of
                # emit_metric and took the whole component down.)
                self.logger.warning(
                    f"metric '{metric.get_name()}' has no measure '{measure_name}'; skipping data point"
                )
                continue
            data_point = {
                "MetricName": measure_name,
                "Dimensions": metric.dimensions_as_collection(False),
                "Timestamp": time.time(),
                "Value": value,
                "Unit": measure.get_unit(),
                "StorageResolution": measure.get_storage_resolution(),
            }
            metric_data.append(data_point)
            if self.metric_config.get_large_fleet_workaround():
                data_point = {
                    "MetricName": measure_name,
                    "Dimensions": metric.dimensions_as_collection(True),
                    "Timestamp": time.time(),
                    "Value": value,
                    "Unit": measure.get_unit(),
                    "StorageResolution": measure.get_storage_resolution(),
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
