"""The Kubernetes-native config source: ``-c CONFIGMAP [mount_dir] [key]``.

Reads the component configuration from a mounted **ConfigMap directory** and hot-reloads it across the
kubelet's atomic ``..data`` symlink swap (DESIGN-subsystems sec 1, FR-CFG-1..5). It is the default
config source on the ``KUBERNETES`` platform and the canonical analogue of
:class:`~edgecommons.config.manager.file_config_manager.FileConfigManager` — it reuses the same
``configuration_changed`` / ``_apply_config`` reload seam, but watches the mount *directory* via
:class:`~edgecommons.utils.directory_watcher.DirectoryWatcher` instead of the file inode.

Mirrors the canonical Java ``ConfigMapConfigProvider``.

**Why not the FILE source?**
A mounted ConfigMap is a directory of symlinks the kubelet swaps atomically. Watching the
user-visible ``config.json`` fires once and dies after the swap (``IN_DELETE_SELF``); worse, the swap
shows up as events on the ``..data`` entry, not on ``config.json``. The directory watcher solves both:
it watches the persistent mount directory, reacts to *any* entry event, and re-arms if the watch is
invalidated (FR-CFG-2).

**Reject-and-keep (FR-CFG-5).**
On a reload, a malformed file (a mid-swap read, or a bad ConfigMap edit) must never crash a running
pod: a parse/read failure is logged and the previous config is kept. The *initial* load still fails
loudly, exactly like the FILE source.

**The subPath caveat (FR-CFG-3).**
A ConfigMap mounted with ``subPath`` is **never** updated by the kubelet — there is no ``..data``
symlink farm and hot-reload is silently dead. This manager warns when it detects a mount with no
``..data`` entry. Mount the whole volume, not a ``subPath``; for a forced ``subPath``/immutable/env
mount use a restart-on-change controller (e.g. Stakater Reloader).

Kubelet projection artifacts (``..data``, ``..2026_...`` timestamped dirs) are never parsed as config:
the configured key is rejected if it is itself such an artifact, reusing the dotfile filter in
:func:`~edgecommons.parameters.source.is_projection_artifact` (FR-CFG-4).
"""

import json
import logging
import os

from watchdog.events import FileSystemEventHandler
from watchdog.observers import Observer

from edgecommons.config.manager.config_manager import ConfigManager
from edgecommons.config.manager.split_config import BaseLayer, resolve_configmap_base
from edgecommons.parameters.source import is_projection_artifact
from edgecommons.utils.directory_watcher import DirectoryWatcher

logger = logging.getLogger("ConfigMapConfigManager")

#: Default ConfigMap mount directory when ``-c CONFIGMAP`` is given no path argument.
DEFAULT_MOUNT_DIR = "/etc/edgecommons"
#: Default config key (file name within the mount) when none is given.
DEFAULT_KEY = "config.json"
#: The kubelet's atomic-swap symlink; its presence indicates a whole-volume (reloadable) mount.
KUBELET_DATA_LINK = "..data"


class ConfigMapConfigManager(ConfigManager):
    """The ``-c CONFIGMAP`` k8s-native config manager (directory-watched, reject-and-keep).

    Args:
        thing_name: the resolved IoT Thing name / identity.
        component_name: the component full name.
        mount_dir: the ConfigMap mount directory, or ``None`` for :data:`DEFAULT_MOUNT_DIR`.
        key: the config file name within the mount, or ``None`` for :data:`DEFAULT_KEY`.

    Raises:
        ValueError: if ``key`` is a kubelet projection artifact (a ``..``/``.`` entry).
    """

    def __init__(
        self,
        thing_name: str,
        component_name: str,
        mount_dir: str = None,
        key: str = None,
        platform=None,
        no_shared_config: bool = False,
    ):
        super().__init__(
            component_name, thing_name, platform=platform, no_shared_config=no_shared_config
        )
        self._mount_dir = mount_dir if mount_dir is not None else DEFAULT_MOUNT_DIR
        self._key = key if key is not None else DEFAULT_KEY
        if is_projection_artifact(self._key):
            raise ValueError(
                "ConfigMap key must not be a kubelet projection artifact (a '..'/'.' entry): "
                + self._key
            )
        self._config_file_path = os.path.join(self._mount_dir, self._key)
        self._config_source = f"ConfigMap (mountDir: {self._mount_dir}, key: {self._key})"
        self._config_provider_family = "CONFIGMAP"
        self._warn_if_subpath_mount()
        # Initial load — fails loudly (parity with FILE) if the key is missing/unreadable.
        self.init()
        self._watcher = DirectoryWatcher(self._mount_dir, self._reload)
        logger.info("Starting ConfigMap directory watcher on %s", self._mount_dir)
        self._watcher.start()
        self._base_file_change_event_handler = ConfigMapBaseFileChangeEventHandler(self)
        self._base_observer = Observer()
        self._base_watched_dirs = set()
        self._sync_base_watch_directories()
        self._base_observer.start()

    def _warn_if_subpath_mount(self) -> None:
        """Warn when the mount appears to be a ``subPath`` (or otherwise non-projected) mount that will
        never hot-reload — detected by the absence of the kubelet ``..data`` symlink (FR-CFG-3)."""
        if not os.path.exists(os.path.join(self._mount_dir, KUBELET_DATA_LINK)):
            logger.warning(
                "ConfigMap mount '%s' has no '%s' symlink — this looks like a subPath/immutable "
                "mount, which the kubelet never updates, so hot-reload is disabled. Mount the whole "
                "volume (not a subPath), or use a restart-on-change controller.",
                self._mount_dir,
                KUBELET_DATA_LINK,
            )

    def _load_configuration(self) -> dict:
        try:
            with open(self._config_file_path) as f:
                return json.load(f)
        except (EnvironmentError, json.JSONDecodeError) as e:
            logger.fatal(
                "Error reading ConfigMap configuration '%s': %s", self._config_file_path, e
            )
            raise RuntimeError(
                f"Error reading ConfigMap configuration '{self._config_file_path}': {e}"
            ) from e

    def _resolve_base_layer(self, component_layer: dict) -> BaseLayer:
        return resolve_configmap_base(self._mount_dir, self._config_file_path, component_layer)

    def _base_watch_directories(self) -> list:
        if self._latest_base_source and os.path.isabs(self._latest_base_source):
            return [os.path.dirname(os.path.abspath(self._latest_base_source))]
        return []

    def _sync_base_watch_directories(self) -> None:
        observer = getattr(self, "_base_observer", None)
        handler = getattr(self, "_base_file_change_event_handler", None)
        if observer is None or handler is None:
            return
        for path_to_watch in self._base_watch_directories():
            resolved = os.path.abspath(path_to_watch)
            if resolved in self._base_watched_dirs:
                continue
            observer.schedule(handler, path=resolved, recursive=False)
            self._base_watched_dirs.add(resolved)

    def _is_relevant_base_path(self, path: str) -> bool:
        if not path or not self._latest_base_source or not os.path.isabs(self._latest_base_source):
            return False
        return os.path.abspath(path) == os.path.abspath(self._latest_base_source)

    def _commit_layers(self, component_layer, base_layer, base_source) -> None:
        super()._commit_layers(component_layer, base_layer, base_source)
        self._sync_base_watch_directories()

    def _reload(self) -> None:
        """Reload callback: re-read the ConfigMap key and apply it. Reject-and-keep on a
        transient/malformed read (a mid-swap window or a bad edit) so a running pod never crashes on
        reload (FR-CFG-5)."""
        logger.info("ConfigMap changed: applying new config from %s", self._config_file_path)
        if not self.reload_from_provider():
            logger.warning("ConfigMap reload failed (keeping previous config).")

    def close(self) -> None:
        """Stop the directory-watcher thread so it does not leak on shutdown."""
        watcher = getattr(self, "_watcher", None)
        if watcher is not None:
            try:
                watcher.stop_thread()
                watcher.join(timeout=5)
            except Exception as e:
                logger.warning("Error stopping ConfigMap directory watcher: %s", e)
            self._watcher = None
        observer = getattr(self, "_base_observer", None)
        if observer is not None:
            try:
                observer.stop()
                observer.join(timeout=5)
            except Exception as e:
                logger.warning("Error stopping ConfigMap shared-base observer: %s", e)
            self._base_observer = None


class ConfigMapBaseFileChangeEventHandler(FileSystemEventHandler):
    def __init__(self, configmap_manager: ConfigMapConfigManager):
        self._configmap_manager = configmap_manager
        super().__init__()

    def _matches(self, path) -> bool:
        return self._configmap_manager._is_relevant_base_path(path)

    def _reload(self) -> None:
        try:
            logger.info("ConfigMap shared base changed: applying new config")
            self._configmap_manager.reload_from_provider()
        except Exception as e:
            logger.error(
                "Failed to reload configuration after shared base change: %s",
                e,
                exc_info=True,
            )

    def on_modified(self, event):
        if event.is_directory:
            return
        if self._matches(event.src_path):
            self._reload()

    def on_created(self, event):
        if event.is_directory:
            return
        if self._matches(event.src_path):
            self._reload()

    def on_moved(self, event):
        if event.is_directory:
            return
        if self._matches(getattr(event, "dest_path", None)):
            self._reload()
