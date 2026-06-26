import logging
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.targets.cloudwatch import CloudWatch
from ggcommons.metrics.targets.cloudwatch_component import CloudWatchComponent
from ggcommons.metrics.targets.messaging import Messaging
from ggcommons.metrics.targets.metric_log import MetricLog
from ggcommons.metrics.targets.prometheus import Prometheus
from ggcommons.platform.resolver import profile_metric_target


class MetricEmitter:
    # Setting up logger
    logger = logging.getLogger(__name__)

    metric_target = None
    metrics = {}
    metric_config = None
    thing_name = ""
    component_name = ""

    # Maps the configured target name (lower-cased) to its target class. Adding a
    # target is a one-line registration rather than another if/elif branch.
    _TARGET_FACTORIES = {
        "messaging": Messaging,
        "log": MetricLog,
        "cloudwatch": CloudWatch,
        "cloudwatchcomponent": CloudWatchComponent,
        # Pull-based target (FR-MET-1): in-process registry served as OpenMetrics text over HTTP;
        # the platform-profile default on KUBERNETES (see _resolve_target).
        "prometheus": Prometheus,
    }

    # The library default when neither config nor the platform profile selects a target.
    _DEFAULT_TARGET = "log"

    @staticmethod
    def init(config_manager: ConfigManager):
        MetricEmitter.logger.info(f"Initializing MetricEmitter for component: {config_manager.get_component_name()}")
        
        MetricEmitter.metric_config = config_manager.get_metric_config()
        MetricEmitter.thing_name = config_manager.get_thing_name()
        MetricEmitter.component_name = config_manager.get_component_name()
        
        MetricEmitter.logger.debug(f"MetricEmitter configuration - thing: {MetricEmitter.thing_name}, component: {MetricEmitter.component_name}")

        if MetricEmitter.metric_target is None:
            namespace = MetricEmitter.metric_config.get_namespace()
            target = MetricEmitter._resolve_target(config_manager)

            MetricEmitter.logger.info(f"Configuring metric target: {target}, namespace: {namespace}")

            factory = MetricEmitter._TARGET_FACTORIES.get(target.lower())
            if factory is None:
                MetricEmitter.logger.warning(
                    f"Invalid metric target '{target}' specified. Defaulting to 'log'"
                )
                target = "log"
                factory = MetricLog
            MetricEmitter.metric_target = factory(config_manager)

            config_manager.add_config_change_listener(MetricEmitter.metric_target)
            MetricEmitter.logger.info(f"MetricEmitter initialized successfully - target: {target}, registered metrics: {len(MetricEmitter.metrics)}")

    @staticmethod
    def _resolve_target(config_manager: ConfigManager) -> str:
        """Resolve the effective metric target by the FR-MET-4 / FR-RT-3 precedence.

        ``explicit metricEmission.target`` (if present in config) ▸ the platform-profile default
        (``prometheus`` on KUBERNETES) ▸ the library default ``log``. The resolved platform is read
        from the config manager (threaded in by the builder), mirroring how the logging-format
        default is threaded — no new resolver->ConfigManager dependency.
        """
        explicit = config_manager.get_metric_config().get_explicit_target()
        if explicit:
            return explicit
        platform = None
        if hasattr(config_manager, "get_platform"):
            platform = config_manager.get_platform()
        profile_default = profile_metric_target(platform)
        if profile_default:
            MetricEmitter.logger.info(
                "No explicit metricEmission.target; using the %s platform-profile default '%s'",
                platform.value if platform is not None else None,
                profile_default,
            )
            return profile_default
        return MetricEmitter._DEFAULT_TARGET

    @staticmethod
    def get_metric_config():
        return MetricEmitter.metric_config

    @staticmethod
    def get_thing_name():
        return MetricEmitter.thing_name

    @staticmethod
    def get_component_name():
        return MetricEmitter.component_name

    @staticmethod
    def define_metric(metric: Metric):
        metric_name = metric.get_name()
        MetricEmitter.logger.info(f"Defining metric: {metric_name} (namespace: {metric.get_namespace()}, measures: {len(metric.get_measures())})")
        MetricEmitter.logger.debug(f"Metric {metric_name} measures: {list(metric.get_measures().keys())}")
        
        MetricEmitter.metrics[metric_name] = metric

        MetricEmitter.logger.debug(f"Total defined metrics: {len(MetricEmitter.metrics)}")

    @staticmethod
    def is_metric_defined(name: str) -> bool:
        """Pure lookup: True if a metric with this name has been defined. Has no
        side effects (does not emit or register anything)."""
        return name in MetricEmitter.metrics

    @staticmethod
    def shutdown():
        """Close the active metric target (releasing any background threads) and
        reset emitter state. Safe to call when not initialized."""
        target = MetricEmitter.metric_target
        if target is not None:
            try:
                target.close()
            except Exception as e:
                MetricEmitter.logger.warning(f"Error closing metric target: {e}")
        MetricEmitter.metric_target = None
        MetricEmitter.metrics = {}

    @staticmethod
    def emit_metric(name, measure_values):
        if name in MetricEmitter.metrics:
            MetricEmitter.logger.debug(f"Emitting metric: {name} with {len(measure_values)} measures")
            MetricEmitter.logger.debug(f"Metric {name} values: {measure_values}")
            MetricEmitter.metric_target.emit_metric(
                MetricEmitter.metrics[name], measure_values
            )
        else:
            MetricEmitter.logger.warning(f"Attempted to emit undefined metric: {name}. Available metrics: {list(MetricEmitter.metrics.keys())}")

    @staticmethod
    def emit_metric_now(name, measure_values):
        if name in MetricEmitter.metrics:
            MetricEmitter.logger.debug(f"Emitting metric immediately: {name} with {len(measure_values)} measures")
            MetricEmitter.logger.debug(f"Metric {name} immediate values: {measure_values}")
            MetricEmitter.metric_target.emit_metric_now(
                MetricEmitter.metrics[name], measure_values
            )
        else:
            MetricEmitter.logger.warning(f"Attempted to emit undefined metric immediately: {name}. Available metrics: {list(MetricEmitter.metrics.keys())}")
