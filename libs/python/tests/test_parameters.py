"""Python parameters tests — mirror the 9 Rust tests in libs/rust/src/parameters/mod.rs.

Each test uses a unique env-var prefix to avoid collisions across the suite.
"""
import os

import pytest

from ggcommons.parameters import (
    DefaultParameterService,
    EnvSource,
    MountedDirSource,
    ParameterError,
    ParameterSource,
    ParamValue,
    open_from_config,
)


def _svc_env(prefix, names) -> DefaultParameterService:
    source = EnvSource(prefix)
    s = DefaultParameterService.with_memory_cache(source, list(names), [])
    s.refresh()
    return s


def test_env_source_round_trips_name_mapping(monkeypatch):
    monkeypatch.setenv("GGTEST_ENV_MYAPP_DB_HOST", "db.example.com")
    monkeypatch.setenv("GGTEST_ENV_MYAPP_DB_POOLSIZE", "8")
    s = _svc_env("GGTEST_ENV_", ["/myapp/db/host", "/myapp/db/poolSize"])
    assert s.get("/myapp/db/host") == "db.example.com"
    assert s.get_int("/myapp/db/poolSize") == 8
    # Missing parameter is None, not an error.
    assert s.get("/myapp/db/missing") is None


def test_typed_accessors_parse(monkeypatch):
    monkeypatch.setenv("GGTEST_TYPED_FLAG", "true")
    monkeypatch.setenv("GGTEST_TYPED_LIST", "a, b ,c")
    monkeypatch.setenv("GGTEST_TYPED_OBJ", '{"k":1}')
    s = _svc_env("GGTEST_TYPED_", ["/flag", "/list", "/obj"])
    assert s.get_bool("/flag") is True
    assert s.get_string_list("/list") == ["a", "b", "c"]
    assert s.get_json("/obj")["k"] == 1


def test_mounted_dir_reads_files_and_marks_secure_paths(tmp_path):
    cfg = tmp_path / "myapp" / "db"
    cfg.mkdir(parents=True)
    (cfg / "host").write_bytes(b"cfg.example.com")
    sec = tmp_path / "secret"
    sec.mkdir(parents=True)
    (sec / "token").write_bytes(b"s3cr3t")
    # K8s projects an internal "..data" symlink dir that must be skipped.
    (tmp_path / "..data").mkdir()

    source = MountedDirSource(str(tmp_path), ["/secret"])
    s = DefaultParameterService.with_memory_cache(source, [], [("/", True)])
    s.refresh()

    assert s.get("/myapp/db/host") == "cfg.example.com"
    assert s.get("/secret/token") == "s3cr3t"
    names = s.names("/")
    assert "/myapp/db/host" in names
    assert "/secret/token" in names
    # The internal ..data entry is not surfaced as a parameter.
    assert not any("..data" in n for n in names)

    # secure_paths flag rides through the source.
    assert source.fetch("/secret/token").secure is True
    assert source.fetch("/myapp/db/host").secure is False


def test_get_by_path_returns_subtree(monkeypatch):
    monkeypatch.setenv("GGTEST_PATH_MYAPP_A", "1")
    monkeypatch.setenv("GGTEST_PATH_MYAPP_B", "2")
    monkeypatch.setenv("GGTEST_PATH_OTHER_C", "3")
    source = EnvSource("GGTEST_PATH_")
    s = DefaultParameterService.with_memory_cache(source, [], [("/myapp", True)])
    s.refresh()
    sub = s.get_by_path("/myapp")
    assert sub.get("/myapp/a") == "1"
    assert sub.get("/myapp/b") == "2"
    assert "/other/c" not in sub


class _FailingSource(ParameterSource):
    """A source that always errors — stands in for an unreachable remote backend."""

    def fetch(self, name):
        raise ParameterError("offline")

    def fetch_by_path(self, path, recursive):
        raise ParameterError("offline")

    def source_id(self):
        return "failing"


def test_offline_refresh_errors_when_cache_empty():
    source = _FailingSource()
    s = DefaultParameterService.with_memory_cache(source, ["/myapp/x"], [])
    # Empty cache + source down => bootstrap-style refresh surfaces the error.
    with pytest.raises(ParameterError):
        s.refresh()
    assert s.stats().refresh_failures == 1
    assert s.get("/myapp/x") is None


def test_offline_refresh_keeps_cached_values_when_source_down(monkeypatch):
    monkeypatch.setenv("GGTEST_OFFLINE_VAL", "cached")
    s = _svc_env("GGTEST_OFFLINE_", ["/val"])
    assert s.get("/val") == "cached"
    # Drop the env var and refresh again: env fetch returns None (not an error), so the
    # already-cached value is retained (offline-first: never clear).
    monkeypatch.delenv("GGTEST_OFFLINE_VAL")
    s.refresh()
    assert s.get("/val") == "cached"


def test_config_open_env_source(monkeypatch):
    monkeypatch.setenv("GGTEST_CFG_MYAPP_REGION", "us-east-1")
    cfg = {
        "source": {"type": "env", "prefix": "GGTEST_CFG_"},
        "bootstrapOnStart": True,
        "refreshIntervalSecs": 0,
        "sync": {"names": ["/myapp/region"]},
    }
    s = open_from_config(cfg)
    assert s.get("/myapp/region") == "us-east-1"
    assert s.stats().source == "env"


def test_path_entry_accepts_string_or_object(monkeypatch):
    monkeypatch.setenv("GGTEST_PE_MYAPP_A", "1")
    monkeypatch.setenv("GGTEST_PE_OTHER_B", "2")
    cfg = {
        "source": {"type": "env", "prefix": "GGTEST_PE_"},
        "refreshIntervalSecs": 0,
        "sync": {"paths": ["/myapp", {"path": "/other", "recursive": False}]},
    }
    s = open_from_config(cfg)
    # Bare string => recursive; object honours its flag. Both resolve to cached values.
    assert s.get("/myapp/a") == "1"
    assert s.get("/other/b") == "2"


def test_lenient_numeric_refresh_interval(monkeypatch):
    # Greengrass delivers numbers as doubles (300.0). open_from_config must accept it without error;
    # a 0 interval keeps the test from spawning a background thread.
    monkeypatch.setenv("GGTEST_LENIENT_X", "v")
    cfg = {
        "source": {"type": "env", "prefix": "GGTEST_LENIENT_"},
        "refreshIntervalSecs": 0.0,
        "sync": {"names": ["/x"]},
    }
    s = open_from_config(cfg)
    assert s.get("/x") == "v"
    # An integer-valued float is accepted (300.0 -> 300) without raising.
    cfg["refreshIntervalSecs"] = 300.0
    from ggcommons.parameters.config import _lenient_int
    assert _lenient_int(300.0, 0) == 300
