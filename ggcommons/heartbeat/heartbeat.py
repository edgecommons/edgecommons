import logging
import threading
import time
from abc import ABC

from ggcommons.config.manager.configuration_change_listener import ConfigurationChangeListener
from ggcommons.heartbeat.heartbeat_monitor import HeartbeatMonitor
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import MessageBuilder
from ggcommons.messaging.messaging_client import MessagingClient

logger = logging.getLogger("Heartbeat")


class Heartbeat(ConfigurationChangeListener, ABC):
    __MESSAGE_NAME = 'heartbeat'
    __MESSAGE_VERSION = '1.0.0'

    def __init__(self, configuration_manager: ConfigManager):
        super().__init__()
        self._heartbeat_thread = None
        self._configuration_manager = configuration_manager
        self._configuration_manager.add_config_change_listener(self)
        self.__compute_topic()
        self.keep_running = True
        self.__run_heartbeat()

    @staticmethod
    def __publish_heartbeat(topic: str, heartbeat_monitor: HeartbeatMonitor, configuration_manager: ConfigManager):
        logger.debug("Publishing heartbeat...")
        data = {}
        cpu_data = heartbeat_monitor.cpu_usage()
        if cpu_data is not None:
            data['cpu'] = cpu_data
        memory_data = heartbeat_monitor.memory_usage()
        if memory_data is not None:
            data['memory'] = memory_data
        disk_data = heartbeat_monitor.disk_usage()
        if disk_data is not None:
            data['disk'] = disk_data
        message = MessageBuilder.build_from_config(name=Heartbeat.__MESSAGE_NAME,
                                                   version=Heartbeat.__MESSAGE_VERSION,
                                                   payload=data,
                                                   config_manager=configuration_manager)
        MessagingClient.publish(topic, message)

    def __compute_topic(self):
        thing_name = self._configuration_manager.get_thing_name()
        component_name = self._configuration_manager.get_component_name()
        self._topic = f"heartbeat/{thing_name}/{component_name}"
        if self._configuration_manager.get_heartbeat_config().get_topic() is not None:
            self._topic = self._configuration_manager.resolve_template(self._configuration_manager.get_heartbeat_config().get_topic())

    @staticmethod
    def __heartbeat_loop(heartbeater, topic: str, configuration_manager: ConfigManager):
        heartbeat_monitor = HeartbeatMonitor(configuration_manager.get_heartbeat_config())
        logger.debug(f"Starting heartbeat using topic: {topic}")
        try:
            while heartbeater.keep_running:
                heartbeater.__publish_heartbeat(topic, heartbeat_monitor, configuration_manager)
                time.sleep(configuration_manager.get_heartbeat_config().get_interval_secs())
        except KeyboardInterrupt:
            logger.error('Publishing loop stopped.')
        except Exception as exc:
            logger.exception(f"Error while publishing heartbeat message: {exc}")

    def __run_heartbeat(self):
        try:
            thread_name = f"{self._configuration_manager.get_component_name()}-heartbeat"
            self._heartbeat_thread = threading.Thread(target=Heartbeat.__heartbeat_loop,
                                                      args=(self, self._topic, self._configuration_manager,),
                                                      name=thread_name)
            self._heartbeat_thread.daemon = True
            self._heartbeat_thread.start()
        except Exception as exc:
            logger.exception("Error while starting heartbeat thread" + str(exc))

    def on_configuration_change(self, configuration) -> bool:
        logger.debug("Configuration changed, restarting heartbeat")
        self.keep_running = False
        self._heartbeat_thread.join()
        self.keep_running = True
        self.__compute_topic()
        self.__run_heartbeat()
        logger.debug("Heartbeat restarted")
        return True
