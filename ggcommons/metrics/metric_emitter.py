import logging
from ggcommons.config.manager.config_manager import ConfigManager
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
        MetricEmitter.metric_config = config_manager.get_metric_config()
        MetricEmitter.thing_name = config_manager.get_thing_name()
        MetricEmitter.component_name = config_manager.get_component_name()

        if MetricEmitter.metric_target is None:
            target = MetricEmitter.metric_config.get_target()
            if target.lower() == "messaging":
                MetricEmitter.metric_target = Messaging(config_manager)
            elif target.lower() == "log":
                MetricEmitter.metric_target = MetricLog(config_manager)
            elif target.lower() == "cloudwatch":
                MetricEmitter.metric_target = CloudWatch(config_manager)
            elif target.lower() == "cloudwatchcomponent":
                MetricEmitter.metric_target = CloudWatchComponent(config_manager)
            else:
                MetricEmitter.logger.warning(f"Invalid metric target '{target}' specified. Defaulting to 'log'")
                target = "log"
                MetricEmitter.metric_target = MetricLog(config_manager)
            config_manager.add_config_change_listener(MetricEmitter.metric_target)
            MetricEmitter.logger.info(f"MetricEmitter initialized with target: {target}")

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
    def define_metric(metric):
        MetricEmitter.metrics[metric.name] = metric

    @staticmethod
    def emit_metric(name, measure_values):
        if name in MetricEmitter.metrics:
            MetricEmitter.metric_target.emit_metric(MetricEmitter.metrics[name], measure_values)
        else:
            MetricEmitter.logger.warning(f"Metric {name} is not defined. Ignoring.")

    @staticmethod
    def emit_metric_now(name, measure_values):
        if name in MetricEmitter.metrics:
            MetricEmitter.metric_target.emit_metric_now(MetricEmitter.metrics[name], measure_values)
        else:
            MetricEmitter.logger.warning(f"Metric {name} is not defined. Ignoring.")