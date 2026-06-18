from abc import ABC, abstractmethod
import logging
from typing import Dict
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)


class MetricTarget(ConfigurationChangeListener, ABC):
    """
    Abstract base class for metric targets.
    """

    def __init__(self, config_manager: ConfigManager):
        super().__init__()
        self.config_manager = config_manager
        self.metric_config = config_manager.get_metric_config()
        self.logger = logging.getLogger(type(self).__name__)

    @abstractmethod
    def emit_metric(self, metric, measure_values: Dict[str, float]):
        """
        Abstract method to emit a metric with given measure values.
        """
        pass

    @abstractmethod
    def emit_metric_now(self, metric, measure_values: Dict[str, float]):
        """
        Abstract method to immediately emit a metric with given measure values.
        """
        pass

    @abstractmethod
    def on_configuration_change(self, configuration) -> bool:
        pass

    def close(self) -> None:
        """Release any resources held by the target (threads, files).

        No-op by default; targets that own background resources (e.g. the
        CloudWatch periodic-flush thread) override this.
        """
        pass
