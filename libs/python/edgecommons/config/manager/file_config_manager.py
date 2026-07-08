import json
import logging
import os

from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from edgecommons.config.manager.config_manager import ConfigManager

logger = logging.getLogger("FileConfigManager")


class FileConfigManager(ConfigManager):
    def __init__(
        self,
        thing_name: str,
        component_name: str,
        config_file_path: str,
        platform=None,
    ):
        super().__init__(component_name, thing_name, platform=platform)
        self._config_file_path = config_file_path
        self._config_source = f"Config File (file name: {config_file_path})"
        self._config_provider_family = "FILE"
        self.init()
        self._file_change_event_handler = ConfigFileChangeEventHandler(
            self, config_file_path
        )
        self._observer = Observer()
        self._watched_dirs = set()
        self._sync_watch_directories()
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

    def _watch_directories(self) -> list:
        return [os.path.dirname(os.path.abspath(self._config_file_path))]

    def _sync_watch_directories(self) -> None:
        observer = getattr(self, "_observer", None)
        handler = getattr(self, "_file_change_event_handler", None)
        if observer is None or handler is None:
            return
        for path_to_watch in self._watch_directories():
            resolved = os.path.abspath(path_to_watch)
            if resolved in self._watched_dirs:
                continue
            observer.schedule(handler, path=resolved, recursive=False)
            self._watched_dirs.add(resolved)

    def _is_relevant_config_path(self, path: str) -> bool:
        if not path:
            return False
        return os.path.abspath(path) == os.path.abspath(self._config_file_path)

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
        if hasattr(self._file_config_manager, "_is_relevant_config_path"):
            return self._file_config_manager._is_relevant_config_path(path)
        return bool(path) and path.endswith(os.path.basename(self._file_path))

    def _reload(self) -> None:
        # Isolate reload failures so a parse error or a transient read during an
        # atomic save-and-rename never kills the observer thread (H9).
        try:
            logger.debug(f"Config file {self._file_path} changed; reloading")
            if hasattr(self._file_config_manager, "reload_from_provider"):
                self._file_config_manager.reload_from_provider()
            else:
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
