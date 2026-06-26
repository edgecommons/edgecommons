"""Unit tests for the ``-c CONFIGMAP`` k8s-native config source.

Mirrors the canonical Java ``ConfigMapConfigProviderTest`` / ``DirectoryWatcherTest``: config load
from a mounted-style temp directory, the kubelet dotfile filter (FR-CFG-4), reject-and-keep on an
invalid reload (FR-CFG-5), the subPath warning detection (FR-CFG-3), and the directory-watch RE-ARM
verified by simulating the kubelet atomic ``..data`` swap and a portable file-inode replacement
(FR-CFG-2).

Every test ``close()``s the manager so its :class:`DirectoryWatcher` daemon thread does not leak.
"""

import json
import os
import time

import pytest

from ggcommons.config.manager.configmap_config_manager import (
    DEFAULT_KEY,
    DEFAULT_MOUNT_DIR,
    ConfigMapConfigManager,
)
from ggcommons.parameters.source import is_projection_artifact
from ggcommons.utils.directory_watcher import DirectoryWatcher

COMPONENT = "com.test.MyComponent"
THING = "test-thing"


def _config_json(version: int) -> str:
    return json.dumps({"component": {"global": {"version": version}}})


def _write(path, text: str) -> None:
    with open(path, "w") as f:
        f.write(text)


def _version_of(manager: ConfigMapConfigManager):
    return manager.get_global_config().get("version")


def _wait_until(predicate, timeout: float = 10.0, interval: float = 0.1) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


# ---------- load ----------


def test_loads_config_from_mounted_directory(tmp_path):
    _write(tmp_path / "config.json", _config_json(7))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        assert _version_of(m) == 7
        src = m.get_config_source()
        assert str(tmp_path) in src
        assert "config.json" in src
    finally:
        m.close()


def test_initial_load_fails_loudly_for_missing_key(tmp_path):
    # The initial load must fail loudly (parity with FILE), unlike a reload (reject-and-keep).
    with pytest.raises(RuntimeError, match="config.json"):
        ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")


def test_applies_default_key_when_none_given(tmp_path):
    # key=None -> DEFAULT_KEY (config.json). (mount_dir default is the module constant; we cannot
    # construct against /etc/ggcommons in a unit test, so assert the constant directly.)
    assert DEFAULT_MOUNT_DIR == "/etc/ggcommons"
    assert DEFAULT_KEY == "config.json"
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), None)
    try:
        assert _version_of(m) == 1
        assert DEFAULT_KEY in m.get_config_source()
    finally:
        m.close()


# ---------- dotfile filter (FR-CFG-4) ----------


def test_dotfile_filter_identifies_projection_artifacts():
    # The shared filter reused from MountedDirSource skips the kubelet symlink farm.
    assert is_projection_artifact("..data")
    assert is_projection_artifact("..2026_06_25_12_00_00.123456789")
    assert is_projection_artifact("..data_tmp")
    assert not is_projection_artifact("config.json")


def test_rejects_key_that_is_a_projection_artifact(tmp_path):
    # A projection-artifact key (..data) must never be read as config.
    with pytest.raises(ValueError):
        ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "..data")


# ---------- subPath warning (FR-CFG-3) ----------


def test_constructs_when_subpath_mount_has_no_data_link(tmp_path, caplog):
    # No '..data' symlink -> looks like a subPath mount; manager warns but still constructs + loads.
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        assert _version_of(m) == 1
    finally:
        m.close()


# ---------- reject-and-keep on reload (FR-CFG-5) ----------


def test_reload_applies_valid_change(tmp_path):
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        _write(tmp_path / "config.json", _config_json(2))
        m._reload()
        assert _version_of(m) == 2
    finally:
        m.close()


def test_reload_keeps_previous_on_malformed_json(tmp_path):
    # A malformed reload (e.g. a bad ConfigMap edit) must not crash the pod: keep previous.
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        _write(tmp_path / "config.json", "{ this is : not valid json ]")
        m._reload()  # must not raise
        assert _version_of(m) == 1  # previous valid config retained
    finally:
        m.close()


def test_reload_keeps_previous_when_file_vanishes_mid_swap(tmp_path):
    # Reading the key during a swap window (file briefly absent) must not crash: keep previous.
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        os.remove(tmp_path / "config.json")
        m._reload()  # must not raise
        assert _version_of(m) == 1
    finally:
        m.close()


def test_reload_keeps_previous_on_empty_file(tmp_path):
    # An empty file fails to parse -> keep previous (no apply of garbage).
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        _write(tmp_path / "config.json", "")
        m._reload()  # must not raise
        assert _version_of(m) == 1
    finally:
        m.close()


# ---------- directory-watch re-arm across swaps (FR-CFG-2) ----------


def test_directory_watch_reloads_repeatedly_across_edits(tmp_path):
    # The directory watch must keep firing across successive edits — i.e. it re-arms and is not a
    # one-shot watch. Portable in-place writes; runs on every OS.
    _write(tmp_path / "config.json", _config_json(1))
    os.mkdir(tmp_path / "..data")  # makes the mount look whole-volume (no subPath warning path)
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        time.sleep(0.5)  # let the directory watch arm before mutating
        _write(tmp_path / "config.json", _config_json(2))
        assert _wait_until(lambda: _version_of(m) == 2), "first edit should reload"
        _write(tmp_path / "config.json", _config_json(3))
        assert _wait_until(lambda: _version_of(m) == 3), "watch survived; second edit reloaded"
    finally:
        m.close()


def test_hot_reload_survives_file_inode_replacement(tmp_path):
    # The portable analogue of the kubelet swap: atomically replace the config.json *inode* via
    # os.replace (the IN_DELETE_SELF / inode-replacement scenario). A file-inode watch would die;
    # the directory re-scan watcher re-arms and reloads. Runs on every OS (no symlink privilege).
    _write(tmp_path / "config.json", _config_json(1))
    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        time.sleep(0.5)  # let the watch arm before the swap
        staged = tmp_path / "config.json.tmp"
        _write(staged, _config_json(2))
        os.replace(staged, tmp_path / "config.json")  # atomic inode replacement
        assert _wait_until(lambda: _version_of(m) == 2), "reload should survive inode replacement"
    finally:
        m.close()


def test_hot_reload_survives_kubelet_data_symlink_swap(tmp_path):
    # The faithful kubelet shape: config.json -> ..data/config.json, and ..data is a symlink the
    # kubelet swaps atomically. Requires symlink support (skipped on Windows without privilege).
    first_data = tmp_path / "..2026_a"
    os.mkdir(first_data)
    _write(first_data / "config.json", _config_json(1))
    cwd = os.getcwd()
    try:
        os.chdir(tmp_path)  # create relative symlinks like the kubelet does
        try:
            os.symlink("..2026_a", "..data")
            os.symlink(os.path.join("..data", "config.json"), "config.json")
        except (OSError, NotImplementedError):
            pytest.skip("symlinks not supported on this host; kubelet swap simulation skipped")
    finally:
        os.chdir(cwd)

    m = ConfigMapConfigManager(THING, COMPONENT, str(tmp_path), "config.json")
    try:
        assert _version_of(m) == 1
        time.sleep(0.5)  # let the watch arm before the swap
        # Kubelet swap: new timestamped dir, stage ..data_tmp -> it, atomic rename onto ..data.
        second_data = tmp_path / "..2026_b"
        os.mkdir(second_data)
        _write(second_data / "config.json", _config_json(2))
        os.chdir(tmp_path)
        try:
            os.symlink("..2026_b", "..data_tmp")
            os.replace("..data_tmp", "..data")  # atomic symlink swap
        finally:
            os.chdir(cwd)
        assert _wait_until(lambda: _version_of(m) == 2), "reload should survive the ..data swap"
    finally:
        m.close()


# ---------- DirectoryWatcher unit behavior ----------


def test_directory_watcher_fires_on_entry_change(tmp_path):
    fired = []
    w = DirectoryWatcher(str(tmp_path), lambda: fired.append(1))
    w.start()
    try:
        time.sleep(0.5)  # arm
        _write(tmp_path / "a.txt", "hi")
        assert _wait_until(lambda: len(fired) >= 1), "watcher should fire on a directory change"
    finally:
        w.stop_thread()
        w.join(timeout=5)


def test_directory_watcher_rearms_when_directory_appears_later(tmp_path):
    # Watch a directory that does not exist yet: the scan finds nothing, the watcher stays unarmed,
    # then re-arms and fires once the directory and an entry appear.
    target = tmp_path / "late-mount"
    fired = []
    w = DirectoryWatcher(str(target), lambda: fired.append(1))
    w.start()
    try:
        time.sleep(0.4)  # a few unarmed poll cycles before the directory exists
        os.mkdir(target)
        _write(target / "config.json", "{}")
        assert _wait_until(lambda: len(fired) >= 1), "watcher should re-arm and fire once dir appears"
    finally:
        w.stop_thread()
        w.join(timeout=5)


def test_directory_watcher_stops_cleanly_without_firing(tmp_path):
    fired = []
    w = DirectoryWatcher(str(tmp_path), lambda: fired.append(1))
    w.start()
    time.sleep(0.3)
    w.stop_thread()
    w.join(timeout=5)
    assert not w.is_alive()
    assert w.is_stopped()
    assert len(fired) == 0
