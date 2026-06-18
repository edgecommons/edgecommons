import logging
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.metrics.metric import Metric
from ggcommons.metrics.targets.cloudwatch import CloudWatch
from ggcommons.metrics.targets.cloudwatch_component import CloudWatchComponent
from ggcommons.metrics.targets.messaging import Messaging
from ggcommons.metrics.targets.metric_log import MetricLog


class MetricEmitter:
    # Setting up logger
    logger = logging.getLogger(__name__)

    metric_target = None
    metrics = {}
    metric_config = None
    thing_name = ""
    component_name = ""

    @staticmethod
    def init(config_manager: ConfigManager):
        MetricEmitter.logger.info(f"Initializing MetricEmitter for component: {config_manager.get_component_name()}")
        
        MetricEmitter.metric_config = config_manager.get_metric_config()
        MetricEmitter.thing_name = config_manager.get_thing_name()
        MetricEmitter.component_name = config_manager.get_component_name()
        
        MetricEmitter.logger.debug(f"MetricEmitter configuration - thing: {MetricEmitter.thing_name}, component: {MetricEmitter.component_name}")

        if MetricEmitter.metric_target is None:
            target = MetricEmitter.metric_config.get_target()
            namespace = MetricEmitter.metric_config.get_namespace()
            
            MetricEmitter.logger.info(f"Configuring metric target: {target}, namespace: {namespace}")
            
            if target.lower() == "messaging":
                MetricEmitter.metric_target = Messaging(config_manager)
            elif target.lower() == "log":
                MetricEmitter.metric_target = MetricLog(config_manager)
            elif target.lower() == "cloudwatch":
                MetricEmitter.metric_target = CloudWatch(config_manager)
            elif target.lower() == "cloudwatchcomponent":
                MetricEmitter.metric_target = CloudWatchComponent(config_manager)
            else:
                MetricEmitter.logger.warning(
                    f"Invalid metric target '{target}' specified. Defaulting to 'log'"
                )
                target = "log"
                MetricEmitter.metric_target = MetricLog(config_manager)
                
            config_manager.add_config_change_listener(MetricEmitter.metric_target)
            MetricEmitter.logger.info(f"MetricEmitter initialized successfully - target: {target}, registered metrics: {len(MetricEmitter.metrics)}")

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
