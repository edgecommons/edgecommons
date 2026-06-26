"""Unit tests for the (deprecated) polling FileWatcher and ConfigFileWatcher.

Deterministic: the change-detection logic is driven directly via
``_check_single_file`` against temp files rather than relying on the poll thread's
timing. DeprecationWarnings are silenced.
"""
import os
import threading
import time
import warnings

import pytest

from ggcommons.utils.file_watcher import (
    FileWatcher,
    ConfigFileWatcher,
    FileChangeHandler,
)


@pytest.fixture(autouse=True)
def _ignore_deprecation():
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        yield


class RecordingHandler(FileChangeHandler):
    def __init__(self):
        self.changed = []
        self.created = []
        self.deleted = []

    def on_file_changed(self, file_path):
        self.changed.append(file_path)

    def on_file_created(self, file_path):
        self.created.append(file_path)

    def on_file_deleted(self, file_path):
        self.deleted.append(file_path)


class TestWatchRegistration:
    def test_watch_file_records_state(self, tmp_path):
        f = tmp_path / "c.json"
        f.write_text("{}")
        w = FileWatcher()
        h = RecordingHandler()
        w.watch_file(str(f), h)
        key = os.path.abspath(str(f))
        assert key in w._watched_files
        assert w._handlers[key] is h

    def test_watch_file_requires_path(self):
        w = FileWatcher()
        with pytest.raises(ValueError):
            w.watch_file("", RecordingHandler())

    def test_watch_file_requires_handler(self, tmp_path):
        f = tmp_path / "c.json"
        f.write_text("{}")
        w = FileWatcher()
        with pytest.raises(ValueError):
            w.watch_file(str(f), None)

    def test_watch_file_missing_raises(self, tmp_path):
        w = FileWatcher()
        with pytest.raises(FileNotFoundError):
            w.watch_file(str(tmp_path / "nope.json"), RecordingHandler())

    def test_unwatch_file(self, tmp_path):
        f = tmp_path / "c.json"
        f.write_text("{}")
        w = FileWatcher()
        h = RecordingHandler()
        w.watch_file(str(f), h)
        w.unwatch_file(str(f))
        assert os.path.abspath(str(f)) not in w._watched_files

    def test_unwatch_empty_path_noop(self):
        FileWatcher().unwatch_file("")


class TestLifecycle:
    def test_start_stop_is_running(self):
        w = FileWatcher(poll_interval=0.01)
        assert w.is_running() is False
        w.start()
        assert w.is_running() is True
        # second start is a no-op
        w.start()
        w.stop()
        assert w.is_running() is False


class TestChangeDetection:
    def _watched(self, tmp_path):
        f = tmp_path / "c.json"
        f.write_text("aaa")
        w = FileWatcher()
        h = RecordingHandler()
        w.watch_file(str(f), h)
        key = os.path.abspath(str(f))
        return w, h, f, key

    def test_modification_fires_changed(self, tmp_path):
        w, h, f, key = self._watched(tmp_path)
        # change content + bump mtime
        f.write_text("bbbbbb")
        os.utime(str(f), (time.time() + 10, time.time() + 10))
        w._check_single_file(key, w._watched_files[key])
        assert h.changed == [key]

    def test_deletion_fires_deleted(self, tmp_path):
        w, h, f, key = self._watched(tmp_path)
        os.remove(str(f))
        w._check_single_file(key, w._watched_files[key])
        assert h.deleted == [key]
        assert w._watched_files[key]["exists"] is False

    def test_recreation_fires_created(self, tmp_path):
        w, h, f, key = self._watched(tmp_path)
        os.remove(str(f))
        w._check_single_file(key, w._watched_files[key])
        # recreate
        f.write_text("new")
        w._check_single_file(key, w._watched_files[key])
        assert h.created == [key]
        assert w._watched_files[key]["exists"] is True

    def test_no_change_no_callback(self, tmp_path):
        w, h, f, key = self._watched(tmp_path)
        w._check_single_file(key, w._watched_files[key])
        assert h.changed == [] and h.created == [] and h.deleted == []

    def test_check_single_file_no_handler_noop(self, tmp_path):
        w, h, f, key = self._watched(tmp_path)
        del w._handlers[key]
        # no handler -> returns early without error
        w._check_single_file(key, w._watched_files[key])

    def test_check_files_iterates(self, tmp_path):
        w, h, f, key = self._watched(tmp_path)
        f.write_text("zzzzzz")
        os.utime(str(f), (time.time() + 20, time.time() + 20))
        w._check_files()
        assert h.changed == [key]

    def test_handler_exception_is_swallowed(self, tmp_path):
        f = tmp_path / "c.json"
        f.write_text("a")
        w = FileWatcher()

        class Boom(FileChangeHandler):
            def on_file_changed(self, file_path):
                raise RuntimeError("boom")

        w.watch_file(str(f), Boom())
        key = os.path.abspath(str(f))
        f.write_text("bbbb")
        os.utime(str(f), (time.time() + 30, time.time() + 30))
        # exception inside handler must not propagate
        w._check_single_file(key, w._watched_files[key])


class TestConfigFileWatcher:
    def test_debounced_change_invokes_callback(self):
        calls = []
        cfw = ConfigFileWatcher("cfg.json", lambda p: calls.append(p), debounce_seconds=0.05)
        cfw.on_file_changed("cfg.json")
        # the debounce timer should fire shortly
        deadline = time.time() + 2
        while not calls and time.time() < deadline:
            time.sleep(0.02)
        assert calls == ["cfg.json"]

    def test_created_triggers_change(self):
        calls = []
        cfw = ConfigFileWatcher("cfg.json", lambda p: calls.append(p), debounce_seconds=0.05)
        cfw.on_file_created("cfg.json")
        deadline = time.time() + 2
        while not calls and time.time() < deadline:
            time.sleep(0.02)
        assert calls == ["cfg.json"]

    def test_deleted_logs_only(self):
        cfw = ConfigFileWatcher("cfg.json", lambda p: None)
        # no exception, no callback
        cfw.on_file_deleted("cfg.json")

    def test_debounce_callback_exception_swallowed(self):
        def boom(p):
            raise RuntimeError("x")

        cfw = ConfigFileWatcher("cfg.json", boom, debounce_seconds=0.01)
        cfw._handle_debounced_change()  # must not raise

    def test_rapid_changes_debounced_to_one(self):
        calls = []
        cfw = ConfigFileWatcher("cfg.json", lambda p: calls.append(p), debounce_seconds=0.1)
        for _ in range(5):
            cfw.on_file_changed("cfg.json")
            time.sleep(0.01)
        deadline = time.time() + 2
        while not calls and time.time() < deadline:
            time.sleep(0.02)
        # only the last debounce timer should have fired
        assert calls == ["cfg.json"]
