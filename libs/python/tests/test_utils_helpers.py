"""Unit tests for edgecommons.utils.Utils and ThreadSafeCounter.

The non-``get_utc_z`` helpers are deprecated but still genuinely testable in-process,
so they are exercised here for real behavior. DeprecationWarnings are silenced.
"""
import warnings

import pytest

from edgecommons.utils.utils import Utils, ThreadSafeCounter


@pytest.fixture(autouse=True)
def _ignore_deprecation():
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        yield


class TestTimeAndSleep:
    def test_get_utc_z_format(self):
        s = Utils.get_utc_z()
        assert s.endswith("Z")
        assert "T" in s

    def test_sleep_noop_for_nonpositive(self):
        # Should return immediately without sleeping.
        Utils.sleep(0)
        Utils.sleep(-5)

    def test_sleep_small_positive(self):
        Utils.sleep(1)  # 1ms


class TestJsonHelpers:
    def test_safe_json_loads_valid(self):
        assert Utils.safe_json_loads('{"a": 1}') == {"a": 1}

    def test_safe_json_loads_empty_returns_default(self):
        assert Utils.safe_json_loads("", default={"d": 1}) == {"d": 1}

    def test_safe_json_loads_invalid_returns_default(self):
        assert Utils.safe_json_loads("{not json", default="fallback") == "fallback"

    def test_safe_json_dumps_valid(self):
        out = Utils.safe_json_dumps({"a": 1})
        assert '"a": 1' in out

    def test_safe_json_dumps_handles_unserializable(self):
        # default=str lets it serialize most things; force a failure with a key type.
        class Bad:
            pass

        # sets are not JSON serializable and default=str will stringify, so use a dict key that fails
        out = Utils.safe_json_dumps({(1, 2): "x"}, default="ERR")
        assert out == "ERR"


class TestFileHelpers:
    def test_ensure_directory_exists(self, tmp_path):
        target = tmp_path / "a" / "b" / "file.txt"
        Utils.ensure_directory_exists(str(target))
        assert (tmp_path / "a" / "b").is_dir()

    def test_ensure_directory_exists_empty_path_noop(self):
        Utils.ensure_directory_exists("")  # no exception

    def test_read_file_safe_roundtrip(self, tmp_path):
        f = tmp_path / "x.txt"
        f.write_text("hello")
        assert Utils.read_file_safe(str(f)) == "hello"

    def test_read_file_safe_missing_returns_none(self, tmp_path):
        assert Utils.read_file_safe(str(tmp_path / "nope.txt")) is None

    def test_write_file_safe_creates_and_writes(self, tmp_path):
        f = tmp_path / "sub" / "out.txt"
        assert Utils.write_file_safe(str(f), "data") is True
        assert f.read_text() == "data"

    def test_write_file_safe_none_content(self, tmp_path):
        assert Utils.write_file_safe(str(tmp_path / "x.txt"), None) is False

    def test_get_file_size(self, tmp_path):
        f = tmp_path / "s.txt"
        f.write_text("12345")
        assert Utils.get_file_size(str(f)) == 5
        assert Utils.get_file_size(str(tmp_path / "missing")) == 0

    def test_is_file_readable(self, tmp_path):
        f = tmp_path / "r.txt"
        f.write_text("x")
        assert Utils.is_file_readable(str(f)) is True
        assert Utils.is_file_readable(str(tmp_path / "no.txt")) is False

    def test_is_file_writable_existing(self, tmp_path):
        f = tmp_path / "w.txt"
        f.write_text("x")
        assert Utils.is_file_writable(str(f)) is True

    def test_is_file_writable_new_in_dir(self, tmp_path):
        assert Utils.is_file_writable(str(tmp_path / "new.txt")) is True

    def test_is_file_writable_no_dir(self):
        assert Utils.is_file_writable("bare.txt") is False


class TestDictHelpers:
    def test_merge_dicts_deep(self):
        a = {"x": {"y": 1}, "z": 2}
        b = {"x": {"w": 3}, "q": 4}
        merged = Utils.merge_dicts(a, b)
        assert merged == {"x": {"y": 1, "w": 3}, "z": 2, "q": 4}

    def test_merge_dicts_shallow_overwrite(self):
        merged = Utils.merge_dicts({"x": {"y": 1}}, {"x": {"w": 3}}, deep_merge=False)
        assert merged == {"x": {"w": 3}}

    def test_merge_dicts_empty_inputs(self):
        assert Utils.merge_dicts({}, {"a": 1}) == {"a": 1}
        assert Utils.merge_dicts({"a": 1}, {}) == {"a": 1}
        assert Utils.merge_dicts({}, {}) == {}

    def test_flatten_dict(self):
        nested = {"a": {"b": {"c": 1}}, "d": 2}
        assert Utils.flatten_dict(nested) == {"a.b.c": 1, "d": 2}

    def test_get_nested_value(self):
        d = {"a": {"b": {"c": 42}}}
        assert Utils.get_nested_value(d, "a.b.c") == 42
        assert Utils.get_nested_value(d, "a.x.c", default="dflt") == "dflt"
        assert Utils.get_nested_value({}, "a") is None
        assert Utils.get_nested_value(d, "") is None

    def test_set_nested_value(self):
        d = {"a": {}}
        Utils.set_nested_value(d, "a.b.c", 99)
        assert d == {"a": {"b": {"c": 99}}}
        # no-op on empty inputs
        Utils.set_nested_value({}, "a.b", 1)
        Utils.set_nested_value({"a": 1}, "", 2)

    def test_validate_required_keys(self):
        assert Utils.validate_required_keys({"a": 1, "b": 2}, ["a", "c"]) == ["c"]
        assert Utils.validate_required_keys({}, ["a", "b"]) == ["a", "b"]


class TestStringHelpers:
    def test_sanitize_filename(self):
        assert Utils.sanitize_filename('a<b>c:d/e') == "abcde"
        assert Utils.sanitize_filename("") == "unnamed"
        assert Utils.sanitize_filename('<>:"') == "unnamed"

    def test_format_bytes(self):
        assert Utils.format_bytes(512) == "512 B"
        assert Utils.format_bytes(2048) == "2.0 KB"
        assert Utils.format_bytes(5 * 1024 ** 2) == "5.0 MB"
        assert Utils.format_bytes(3 * 1024 ** 3) == "3.0 GB"

    def test_format_duration(self):
        assert Utils.format_duration(5.0) == "5.0s"
        assert "m" in Utils.format_duration(125.0)
        assert "h" in Utils.format_duration(3725.0)


class TestThreadSafeCounter:
    def test_increment_decrement(self):
        c = ThreadSafeCounter(10)
        assert c.increment() == 11
        assert c.increment(4) == 15
        assert c.decrement() == 14
        assert c.decrement(4) == 10
        assert c.get() == 10

    def test_set_and_reset(self):
        c = ThreadSafeCounter()
        c.set(7)
        assert c.get() == 7
        assert c.reset() == 7
        assert c.get() == 0
