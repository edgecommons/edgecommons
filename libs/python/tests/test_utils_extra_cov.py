"""Extra coverage for the error/edge branches of ``ggcommons.utils.utils.Utils``.

These deprecated helpers each have an error-handling branch that the existing suite does not
exercise. The tests below drive those branches deterministically (no real sleeps, no OS-specific
filesystem timing): the interrupt path of ``sleep``, the ``OSError`` path of
``ensure_directory_exists``, the decode/encode failure paths of ``read_file_safe`` /
``write_file_safe``, and the ``OSError`` path of ``get_file_size``.

Every helper is wrapped with ``@deprecated`` so each call also emits a ``DeprecationWarning`` —
asserted via ``pytest.warns`` so the warning machinery is covered too.
"""
import os
import time

import pytest

from ggcommons.utils.utils import Utils


class TestSleepInterrupt:
    """utils.py lines 48-50: ``time.sleep`` raising ``KeyboardInterrupt`` is logged and re-raised."""

    def test_sleep_reraises_keyboard_interrupt(self, monkeypatch):
        def _interrupt(_seconds):
            raise KeyboardInterrupt()

        monkeypatch.setattr(time, "sleep", _interrupt)
        with pytest.warns(DeprecationWarning), pytest.raises(KeyboardInterrupt):
            # milliseconds > 0 so we reach the try/sleep block.
            Utils.sleep(10)


class TestEnsureDirectoryErrors:
    """utils.py lines 87-89: ``os.makedirs`` raising ``OSError`` is logged and re-raised."""

    def test_makedirs_oserror_reraised(self, monkeypatch, tmp_path):
        def _boom(*_args, **_kwargs):
            raise OSError("permission denied")

        monkeypatch.setattr(os, "makedirs", _boom)
        # dirname is a not-yet-existing subdir, so makedirs is attempted (and our stub raises).
        target = str(tmp_path / "missing_dir" / "file.txt")
        with pytest.warns(DeprecationWarning), pytest.raises(OSError):
            Utils.ensure_directory_exists(target)


class TestReadFileSafeErrors:
    """utils.py lines 101-103: a decode failure is swallowed and ``None`` returned."""

    def test_invalid_utf8_returns_none(self, tmp_path):
        bad = tmp_path / "bad.bin"
        # 0xFF is not a valid UTF-8 lead byte -> UnicodeDecodeError on read.
        bad.write_bytes(b"\xff\xff\xff")
        with pytest.warns(DeprecationWarning):
            result = Utils.read_file_safe(str(bad))
        assert result is None


class TestWriteFileSafeErrors:
    """utils.py lines 120-122: an encode failure is swallowed and ``False`` returned."""

    def test_unencodable_content_returns_false(self, tmp_path):
        out = tmp_path / "out.txt"
        # 'é' (U+00E9) cannot be encoded as ASCII -> UnicodeEncodeError on write.
        with pytest.warns(DeprecationWarning):
            ok = Utils.write_file_safe(str(out), "café", encoding="ascii")
        assert ok is False


class TestGetFileSizeErrors:
    """utils.py lines 130-131: ``os.path.getsize`` raising ``OSError`` yields 0."""

    def test_getsize_oserror_returns_zero(self, monkeypatch):
        def _boom(_path):
            raise OSError("stat failed")

        # exists() must be truthy so the getsize() call (which raises) is reached.
        monkeypatch.setattr(os.path, "exists", lambda _p: True)
        monkeypatch.setattr(os.path, "getsize", _boom)
        with pytest.warns(DeprecationWarning):
            assert Utils.get_file_size("anything") == 0
