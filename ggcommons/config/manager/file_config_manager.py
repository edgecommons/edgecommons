import json
import logging
import os

# import time
from abc import ABC

from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("FileConfigManager")


class FileConfigManager(ConfigManager, ABC):
    def __init__(self, component_name, config_file_path):
        super().__init__(component_name)
        self._config_file_path = config_file_path
        self.init()
        path_to_watch = os.path.dirname(os.path.abspath(self._config_file_path))
        self._file_change_event_handler = ConfigFileChangeEventHandler(
            self, config_file_path
        )
        self._observer = Observer()
        self._observer.schedule(
            self._file_change_event_handler, path=path_to_watch, recursive=False
        )
        logger.info("Starting file change observer")
        self._observer.start()

    def _load_configuration(self) -> dict:
        try:
            with open(self._config_file_path) as f:
                return json.load(f)
        except EnvironmentError:
            logger.fatal(f"Unable to open config file at {self._config_file_path}")
            exit(1)

    def get_config_source(self) -> str:
        return f"Config File (file name: {self._config_file_path})"


class ConfigFileChangeEventHandler(FileSystemEventHandler):
    def __init__(self, file_config_manager: FileConfigManager, file_path: str):
        self._file_config_manager = file_config_manager
        self._file_path = file_path
        super().__init__()

    def on_modified(self, event):
        if event.is_directory:
            return None
        elif event.src_path.endswith(os.path.basename(self._file_path)):
            logger.debug(f"Config file {self._file_path} has been modified")
            new_config = self._file_config_manager._load_configuration()
            self._file_config_manager.configuration_changed(new_config)
