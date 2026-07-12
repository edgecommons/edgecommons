import json
import logging
import copy


class MetricConfiguration:
    # Default configuration values
    DEFAULT_CLOUDWATCH_COMPONENT_TOPIC = "cloudwatch/metric/put"
    DEFAULT_TARGET = "log"
    DEFAULT_METRIC_NAMESPACE = "edgecommons"
    DEFAULT_METRIC_FILE_NAME_TEMPLATE = (
        "/greengrass/v2/logs/{ComponentFullName}_metric.log"
    )
    DEFAULT_INTERVAL_SECS = 5
    DEFAULT_MESSAGING_DESTINATION = "ipc"
    DEFAULT_LARGE_FLEET_WORKAROUND = False
    # Rotation for the `log` target (parity with Java/Rust/TS, which rotate at maxFileSize).
    DEFAULT_METRIC_MAX_FILE_SIZE = "10MB"
    DEFAULT_METRIC_BACKUP_COUNT = 5
    # Defaults for the pull-based `prometheus` target (FR-MET-1; mirror the canonical schema).
    DEFAULT_PROMETHEUS_PORT = 9090
    DEFAULT_PROMETHEUS_PATH = "/metrics"

    def __init__(self, json_config=None):
        self.logger = logging.getLogger(self.__class__.__name__)

        # Default values
        self._target = self.DEFAULT_TARGET
        # The raw `target` value as it appeared in config, or None when the section/key was absent.
        # Distinguishing "absent" from an explicit "log" lets MetricEmitter apply the FR-MET-4
        # precedence (explicit config ▸ platform-profile default ▸ library default `log`).
        self._explicit_target = None
        self._namespace = self.DEFAULT_METRIC_NAMESPACE
        self._log_file_name_template = self.DEFAULT_METRIC_FILE_NAME_TEMPLATE
        # The raw `logFileName` exactly as it appeared in config, or None when absent. Distinguishing
        # "absent" from an explicit value lets the `log` target apply the HOST-aware path precedence
        # (explicit config ▸ platform-profile default ▸ library default) — mirroring _explicit_target.
        self._explicit_log_file_name = None
        # UNS-CANONICAL-DESIGN §4.3 / D-U9: the messaging target's topic is no longer
        # configurable (targetConfig.topic is removed from the schema) — the Messaging
        # target builds the UNS metric topic
        # ecv1/{device}/{component}/main/metric/{metricName} itself. Only the
        # cloudwatchcomponent target carries a (fixed) topic.
        self._topic = None
        self._interval_secs = self.DEFAULT_INTERVAL_SECS
        self._destination = self.DEFAULT_MESSAGING_DESTINATION
        self._large_fleet_workaround = self.DEFAULT_LARGE_FLEET_WORKAROUND
        self._max_file_size = self.DEFAULT_METRIC_MAX_FILE_SIZE
        self._backup_count = self.DEFAULT_METRIC_BACKUP_COUNT
        # Raw `buffer` object from the cloudwatch targetConfig (durable store-and-forward buffer for
        # the direct CloudWatch target). None => no buffer section => in-memory batching path.
        self._cloudwatch_buffer = None
        # Pull-based `prometheus` target HTTP exposition (FR-MET-1).
        self._prometheus_port = self.DEFAULT_PROMETHEUS_PORT
        self._prometheus_path = self.DEFAULT_PROMETHEUS_PATH

        if json_config:
            self._explicit_target = json_config.get("target")
            self._target = json_config.get("target", self._target)
            self._namespace = json_config.get("namespace", self._namespace)
            self._large_fleet_workaround = json_config.get(
                "largeFleetWorkaround", self._large_fleet_workaround
            )

            target = self._target.lower()
            target_config = json_config.get("targetConfig", {})

            # Parse the prometheus port/path unconditionally when present: on KUBERNETES the
            # effective target can be prometheus via the platform-profile default even when the
            # config omits `target` (so `target` is still "log" here), and these keys are
            # prometheus-only so reading them is harmless for the other targets.
            self._prometheus_port = int(
                target_config.get("port", self.DEFAULT_PROMETHEUS_PORT)
            )
            self._prometheus_path = target_config.get(
                "path", self.DEFAULT_PROMETHEUS_PATH
            )

            if target == "log":
                self._explicit_log_file_name = target_config.get("logFileName")
                self._log_file_name_template = target_config.get(
                    "logFileName", self.DEFAULT_METRIC_FILE_NAME_TEMPLATE
                )
                self._max_file_size = target_config.get(
                    "maxFileSize", self.DEFAULT_METRIC_MAX_FILE_SIZE
                )

            # §4.3 / D-U9: only the destination survives for the messaging target; the
            # legacy targetConfig.topic override is removed (the topic is the UNS
            # metric topic, built per-metric by the target).
            if target == "messaging":
                self._destination = target_config.get("destination", self._destination)

            # The cloudwatchcomponent topic is the external AWS Greengrass component
            # contract (cloudwatch/metric/put, D-U21) — fixed, no override.
            if target == "cloudwatchcomponent":
                self._topic = self.DEFAULT_CLOUDWATCH_COMPONENT_TOPIC

            if target == "cloudwatch":
                cw_config = target_config.get("cloudwatch", target_config)
                self._interval_secs = int(
                    cw_config.get("intervalSecs", self._interval_secs)
                )
                if self._interval_secs < 1:
                    self._interval_secs = self.DEFAULT_INTERVAL_SECS
                buffer = cw_config.get("buffer")
                if isinstance(buffer, dict):
                    self._cloudwatch_buffer = copy.deepcopy(buffer)

            self.logger.debug(
                f"Metric configuration: target={self._target}, namespace={self._namespace}, logFileName={self._log_file_name_template}, topic={self._topic}, intervalSecs={self._interval_secs}"
            )

    def to_dict(self):
        config = {"target": self._target, "targetConfig": {}}

        if self._target == "messaging":
            config["targetConfig"] = {
                "destination": self._destination,
            }
        elif self._target == "cloudwatchcomponent":
            config["targetConfig"] = {}
        elif self._target == "cloudwatch":
            config["targetConfig"] = {"intervalSecs": self._interval_secs}
        elif self._target == "prometheus":
            config["targetConfig"] = {
                "port": self._prometheus_port,
                "path": self._prometheus_path,
            }
        elif self._target == "log":
            config["targetConfig"] = {"filename": self._log_file_name_template}

        return config

    def __str__(self):
        return json.dumps(self.to_dict())

    def get_target(self) -> str:
        """The effective target token, defaulting to ``log`` when the config omits it.

        Note this collapses "absent" and an explicit ``"log"`` to the same value; use
        :meth:`get_explicit_target` when the distinction matters (FR-MET-4 precedence).
        """
        return self._target

    def get_explicit_target(self):
        """The raw ``metricEmission.target`` exactly as configured, or ``None`` when absent.

        Lets :class:`~edgecommons.metrics.metric_emitter.MetricEmitter` distinguish an explicit
        ``"log"`` (which must win) from an unset target (which falls through to the platform-profile
        default — prometheus on KUBERNETES) per the FR-MET-4 / FR-RT-3 precedence.
        """
        return self._explicit_target

    def get_prometheus_port(self) -> int:
        """HTTP port for the ``prometheus`` target's ``/metrics`` endpoint (default 9090)."""
        return self._prometheus_port

    def get_prometheus_path(self) -> str:
        """HTTP path for the ``prometheus`` target's OpenMetrics exposition (default ``/metrics``)."""
        return self._prometheus_path

    def get_namespace(self) -> str:
        return self._namespace

    def get_log_file_name_template(self) -> str:
        return self._log_file_name_template

    def get_explicit_log_file_name(self):
        """The raw ``metricEmission.targetConfig.logFileName`` exactly as configured, or ``None`` when
        absent. Lets the metric ``log`` target distinguish an explicit path (which must win) from an
        unset one (which falls through to the platform-profile default, then the library default) —
        mirroring :meth:`get_explicit_target` for the target."""
        return self._explicit_log_file_name

    def get_topic(self) -> str:
        """The fixed topic of the ``cloudwatchcomponent`` target
        (``cloudwatch/metric/put``, the external AWS Greengrass component contract —
        D-U21), or ``None`` for every other target. The ``messaging`` target no longer
        carries a configured topic: it publishes to the UNS metric topic
        ``ecv1/{device}/{component}/main/metric/{metricName}`` (§4.3)."""
        return self._topic

    def get_interval_secs(self) -> int:
        return self._interval_secs

    def get_cloudwatch_buffer(self):
        """Raw `buffer` object from the cloudwatch targetConfig, or None when absent.

        Present => the durable store-and-forward buffer is configured for the direct CloudWatch
        target (`type: durable|memory`, `path`, `maxDiskBytes`, `onFull`, `fsync`). Absent => the
        legacy in-memory batching path.
        """
        return copy.deepcopy(self._cloudwatch_buffer)

    def get_destination(self) -> str:
        return self._destination

    def get_large_fleet_workaround(self) -> bool:
        return self._large_fleet_workaround

    def get_max_file_size(self) -> str:
        return self._max_file_size

    def get_backup_count(self) -> int:
        return self._backup_count
