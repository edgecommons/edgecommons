import abc


class ConfigurationChangeListener(metaclass=abc.ABCMeta):

    def __init__(self):
        pass

    @abc.abstractmethod
    def on_configuration_change(self, configuration) -> bool:
        pass