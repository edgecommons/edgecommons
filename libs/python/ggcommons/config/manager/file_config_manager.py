import json
import logging
import os

from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from ggcommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("FileConfigManager")


class FileConfigManager(ConfigManager):
    def __init__(self, thing_name: str, component_name: str, config_file_path: str):
        super().__init__(component_name, thing_name)
        self._config_file_path = config_file_path
        self._config_source = f"Config File (file name: {config_file_path})"
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
        except EnvironmentError as e:
            logger.fatal(f"Unable to open config file at {self._config_file_path}")
            raise RuntimeError(
                f"Unable to open config file at {self._config_file_path}"
            ) from e

    def close(self) -> None:
        """Stop the file-change observer thread so it does not leak on shutdown."""
        observer = getattr(self, "_observer", None)
        if observer is not None:
            try:
                observer.stop()
                observer.join(timeout=5)
            except Exception as e:
                logger.warning(f"Error stopping config file observer: {e}")
            self._observer = None


class ConfigFileChangeEventHandler(FileSystemEventHandler):
    def __init__(self, file_config_manager: FileConfigManager, file_path: str):
        self._file_config_manager = file_config_manager
        self._file_path = file_path
        super().__init__()

    def _matches(self, path) -> bool:
        return bool(path) and path.endswith(os.path.basename(self._file_path))

    def _reload(self) -> None:
        # Isolate reload failures so a parse error or a transient read during an
        # atomic save-and-rename never kills the observer thread (H9).
        try:
            logger.debug(f"Config file {self._file_path} changed; reloading")
            new_config = self._file_config_manager._load_configuration()
            self._file_config_manager.configuration_changed(new_config)
        except Exception as e:
            logger.error(
                f"Failed to reload configuration after change to {self._file_path}: {e}",
                exc_info=True,
            )

    def on_modified(self, event):
        if event.is_directory:
            return
        if self._matches(event.src_path):
            self._reload()

    def on_created(self, event):
        # Editors/config writers often write a temp file then rename it onto the
        # target, which surfaces as a create (or move) rather than a modify.
        if event.is_directory:
            return
        if self._matches(event.src_path):
            self._reload()

    def on_moved(self, event):
        if event.is_directory:
            return
        if self._matches(getattr(event, "dest_path", None)):
            self._reload()
