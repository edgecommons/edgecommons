import logging
import json
import os
import re
from logging.handlers import RotatingFileHandler
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.targets.emf_helper import build_metric_data_emf
from ggcommons.metrics.targets.metric_target import MetricTarget
from ggcommons.platform.resolver import profile_metric_log_path

# Map size suffixes -> bytes (parity with the Java/Rust/TS log targets, which rotate at maxFileSize).
_SIZE_UNITS = {"": 1, "B": 1, "KB": 1024, "MB": 1024 ** 2, "GB": 1024 ** 3}


def _parse_size(text: str, default_bytes: int = 10 * 1024 ** 2) -> int:
    """Parse a human size like '10MB'/'512KB'/'1GB' into bytes; fall back to default on garbage."""
    if not text:
        return default_bytes
    m = re.fullmatch(r"\s*(\d+)\s*([KMGT]?B|)\s*", str(text), re.IGNORECASE)
    if not m:
        return default_bytes
    return int(m.group(1)) * _SIZE_UNITS.get(m.group(2).upper(), 1)


class MetricLog(MetricTarget):
    def __init__(self, config_manager: ConfigManager):
        super().__init__(config_manager)
        # Fail-soft: stays False until a file handler successfully opens. When the metric-log file
        # cannot be opened (e.g. HOST has no writable /greengrass/v2/logs), the target warns and drops
        # file metrics rather than aborting GGCommons init — matching the Java/Rust/TS log targets.
        self._enabled = False
        self._configure_logger()

    def _resolve_log_file_path(self) -> str:
        """Resolve the metric-log file path with the HOST-aware precedence: explicit
        ``metricEmission.targetConfig.logFileName`` config ▸ the platform-profile default (a local
        path on HOST/KUBERNETES, which lack ``/greengrass/v2/logs``) ▸ the library default. The
        resulting template is run through ``resolve_template`` for ``{ComponentFullName}`` etc."""
        metric_config = self.config_manager.get_metric_config()
        template = metric_config.get_explicit_log_file_name()
        if not template:
            platform = (
                self.config_manager.get_platform()
                if hasattr(self.config_manager, "get_platform")
                else None
            )
            template = profile_metric_log_path(platform) or metric_config.get_log_file_name_template()
        return self.config_manager.resolve_template(template)

    def _configure_logger(self):
        self.metric_logger = logging.getLogger("metric_file")
        self.metric_logger.setLevel(logging.INFO)
        # Rebuild handlers from scratch so a hot reload doesn't stack duplicate handlers.
        for h in list(self.metric_logger.handlers):
            self.metric_logger.removeHandler(h)
            try:
                h.close()
            except Exception:
                pass
        self._enabled = False
        metric_config = self.config_manager.get_metric_config()
        log_file_path = self._resolve_log_file_path()
        # Rotate at maxFileSize, keeping backup_count backups (parity with the other languages).
        max_bytes = _parse_size(metric_config.get_max_file_size())
        try:
            # Create the parent directory when possible, then open the rotating handler.
            parent = os.path.dirname(log_file_path)
            if parent:
                os.makedirs(parent, exist_ok=True)
            handler = RotatingFileHandler(
                log_file_path, maxBytes=max_bytes, backupCount=metric_config.get_backup_count()
            )
        except OSError as e:
            # Fail-soft (parity with Java/Rust/TS): warn and drop file metrics, never abort init.
            self.logger.warning(
                "metric log: cannot open '%s'; dropping metrics (fail-soft): %s",
                log_file_path,
                e,
            )
            return
        formatter = logging.Formatter("%(message)s")  # EMF metrics are in JSON format
        handler.setFormatter(formatter)
        self.metric_logger.addHandler(handler)
        self.metric_logger.propagate = False
        self._enabled = True

    def emit_metric_now(self, metric, measure_values):
        if not self._enabled:
            return  # file metrics disabled (the handler failed to open) — fail-soft
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
