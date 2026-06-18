"""Unit tests for Tier D deprecations (no broker / AWS required)."""
import warnings

import pytest

from ggcommons.utils.utils import Utils, ThreadSafeCounter
from ggcommons.utils.file_watcher import FileWatcher, ConfigFileWatcher


def test_unused_utils_methods_warn():
    with pytest.warns(DeprecationWarning):
        Utils.format_bytes(2048)
    with pytest.warns(DeprecationWarning):
        Utils.merge_dicts({"a": 1}, {"b": 2})


def test_get_utc_z_is_not_deprecated():
    # The one used Utils method must NOT emit a deprecation warning.
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        assert Utils.get_utc_z()


def test_thread_safe_counter_warns():
    with pytest.warns(DeprecationWarning):
        counter = ThreadSafeCounter()
    # Still functional after the warning.
    assert counter.increment() == 1


def test_file_watcher_classes_warn():
    with pytest.warns(DeprecationWarning):
        FileWatcher()
    with pytest.warns(DeprecationWarning):
        ConfigFileWatcher("cfg.json", lambda p: None)
