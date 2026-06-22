"""Python parameters tests — mirror the 9 Rust tests in libs/rust/src/parameters/mod.rs,
plus coverage tests bringing every source file to >90% (mirroring the credentials bar).

Each test uses a unique env-var prefix to avoid collisions across the suite.
"""
import os
import threading
import time

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


# ---------------------------------------------------------------------------
# source.py — ParamValue, EnvSource, MountedDirSource edge branches.
# ---------------------------------------------------------------------------


def test_param_value_repr_never_renders_bytes():
    # __repr__ must redact the raw bytes (a value may be secure).
    pv = ParamValue(b"super-secret-bytes", secure=True, version="7")
    r = repr(pv)
    assert "super-secret-bytes" not in r
    assert "secure=True" in r
    assert "redacted" in r
    assert "version='7'" in r


def test_mounted_dir_fetch_missing_returns_none(tmp_path):
    source = MountedDirSource(str(tmp_path), [])
    assert source.fetch("/does/not/exist") is None
    assert source.source_id() == "mountedDir"


def test_mounted_dir_fetch_on_directory_returns_none(tmp_path):
    # A directory at the parameter name is "not a parameter".
    (tmp_path / "myapp").mkdir()
    source = MountedDirSource(str(tmp_path), [])
    assert source.fetch("/myapp") is None


def test_mounted_dir_fetch_single_value(tmp_path):
    (tmp_path / "region").write_bytes(b"us-east-1")
    source = MountedDirSource(str(tmp_path), [])
    pv = source.fetch("/region")
    assert pv is not None
    assert pv.value == b"us-east-1"
    assert pv.secure is False


def test_mounted_dir_handles_non_utf8_value(tmp_path):
    # A non-UTF-8 value is fetched as raw bytes; get() (UTF-8) raises, get_bytes() returns it.
    (tmp_path / "blob").write_bytes(b"\xff\xfe\x00")
    source = MountedDirSource(str(tmp_path), [])
    s = DefaultParameterService.with_memory_cache(source, [], [("/", True)])
    s.refresh()
    assert s.get_bytes("/blob") == b"\xff\xfe\x00"
    with pytest.raises(ParameterError):
        s.get("/blob")
    # get_by_path skips the non-UTF-8 entry rather than raising.
    assert "/blob" not in s.get_by_path("/")


def test_mounted_dir_missing_base_dir_yields_empty(tmp_path):
    # fetch_by_path over a non-existent base directory returns [] (FileNotFoundError in _walk).
    source = MountedDirSource(str(tmp_path / "nope"), [])
    assert source.fetch_by_path("/", True) == []


def test_mounted_dir_non_recursive_skips_subdirs(tmp_path):
    (tmp_path / "top").write_bytes(b"1")
    sub = tmp_path / "deep"
    sub.mkdir()
    (sub / "inner").write_bytes(b"2")
    source = MountedDirSource(str(tmp_path), [])
    names = {n for n, _ in source.fetch_by_path("/", recursive=False)}
    assert "/top" in names
    assert "/deep/inner" not in names


def test_mounted_dir_walk_oserror_is_wrapped(tmp_path, monkeypatch):
    # An OSError (not FileNotFoundError) while listing a dir is wrapped as ParameterError.
    (tmp_path / "f").write_bytes(b"x")
    source = MountedDirSource(str(tmp_path), [])
    real_listdir = os.listdir

    def boom(path):
        raise PermissionError("denied")

    monkeypatch.setattr(os, "listdir", boom)
    with pytest.raises(ParameterError):
        source.fetch_by_path("/", recursive=True)
    monkeypatch.setattr(os, "listdir", real_listdir)


def test_mounted_dir_walk_read_oserror_is_wrapped(tmp_path, monkeypatch):
    # An OSError while reading a file during a walk is wrapped as ParameterError.
    (tmp_path / "f").write_bytes(b"x")
    source = MountedDirSource(str(tmp_path), [])
    import ggcommons.parameters.source as srcmod
    real_open = srcmod.open if hasattr(srcmod, "open") else open

    def boom_open(path, *a, **k):
        raise PermissionError("denied")

    monkeypatch.setattr("builtins.open", boom_open)
    with pytest.raises(ParameterError):
        source.fetch_by_path("/", recursive=True)
    monkeypatch.setattr("builtins.open", real_open)


def test_mounted_dir_fetch_isadirectory_returns_none(tmp_path, monkeypatch):
    # Race/platform quirk: os.path.isdir() says "not a dir" but open() raises IsADirectoryError.
    # Treated as "not a parameter" (None), not an error.
    (tmp_path / "f").write_bytes(b"x")
    source = MountedDirSource(str(tmp_path), [])
    monkeypatch.setattr(os.path, "isdir", lambda p: False)

    def raise_isdir(path, *a, **k):
        raise IsADirectoryError("is a dir")

    monkeypatch.setattr("builtins.open", raise_isdir)
    assert source.fetch("/f") is None


def test_mounted_dir_fetch_read_oserror_is_wrapped(tmp_path, monkeypatch):
    # An OSError while reading a single file in fetch() is wrapped as ParameterError.
    (tmp_path / "f").write_bytes(b"x")
    source = MountedDirSource(str(tmp_path), [])

    def boom_open(path, *a, **k):
        raise PermissionError("denied")

    monkeypatch.setattr("builtins.open", boom_open)
    with pytest.raises(ParameterError):
        source.fetch("/f")


# ---------------------------------------------------------------------------
# service.py — typed accessors (all branches), get_by_path/get_bytes None,
# background refresh thread, path-refresh failure, close().
# ---------------------------------------------------------------------------


def test_typed_accessor_branches(monkeypatch):
    monkeypatch.setenv("GGTEST_TB_FALSEFLAG", "false")
    monkeypatch.setenv("GGTEST_TB_EMPTYLIST", "")
    monkeypatch.setenv("GGTEST_TB_NOTINT", "abc")
    monkeypatch.setenv("GGTEST_TB_NOTBOOL", "maybe")
    s = _svc_env("GGTEST_TB_", ["/falseflag", "/emptylist", "/notint", "/notbool"])
    assert s.get_bool("/falseflag") is False
    assert s.get_string_list("/emptylist") == []
    with pytest.raises(ParameterError):
        s.get_int("/notint")
    with pytest.raises(ParameterError):
        s.get_bool("/notbool")
    # Missing parameters return None across all typed accessors (no error).
    assert s.get_int("/missing") is None
    assert s.get_bool("/missing") is None
    assert s.get_json("/missing") is None
    assert s.get_string_list("/missing") is None
    assert s.get_bytes("/missing") is None


def test_get_json_invalid_raises(monkeypatch):
    monkeypatch.setenv("GGTEST_JSONBAD_X", "{not json")
    s = _svc_env("GGTEST_JSONBAD_", ["/x"])
    with pytest.raises(ParameterError):
        s.get_json("/x")


class _PathFailingSource(ParameterSource):
    """fetch() works (so the name caches) but fetch_by_path() always errors."""

    def fetch(self, name):
        return ParamValue.plain(b"ok")

    def fetch_by_path(self, path, recursive):
        raise ParameterError("path offline")

    def source_id(self):
        return "pathfailing"


def test_path_refresh_failure_is_non_fatal_when_cache_nonempty():
    # A name caches successfully; the failing path refresh increments failures but does not raise
    # because the cache is non-empty (offline-first).
    s = DefaultParameterService.with_memory_cache(
        _PathFailingSource(), ["/seed"], [("/sub", True)]
    )
    s.refresh()
    assert s.get("/seed") == "ok"
    assert s.stats().refresh_failures == 1


def test_background_refresh_thread_observes_source_change():
    # A mutable source: the background thread re-pulls and the new value appears in the cache.
    class _MutSource(ParameterSource):
        def __init__(self):
            self.val = b"v1"

        def fetch(self, name):
            return ParamValue.plain(self.val)

        def fetch_by_path(self, path, recursive):
            return []

        def source_id(self):
            return "mut"

    src = _MutSource()
    s = DefaultParameterService.with_memory_cache(src, ["/k"], [])
    s.with_refresh(1)  # 1s interval background daemon
    try:
        s.refresh()
        assert s.get("/k") == "v1"
        src.val = b"v2"
        # Wait for the background thread to observe the change.
        deadline = time.time() + 5
        while time.time() < deadline and s.get("/k") != "v2":
            time.sleep(0.1)
        assert s.get("/k") == "v2"
    finally:
        s.close()
    # close() is idempotent.
    s.close()


def test_stats_reflects_refresh_age(monkeypatch):
    monkeypatch.setenv("GGTEST_STATS_X", "1")
    s = _svc_env("GGTEST_STATS_", ["/x"])
    st = s.stats()
    assert st.parameter_count == 1
    assert st.source == "env"
    assert st.last_refresh_age_ms is not None
    assert st.last_refresh_age_ms >= 0


# ---------------------------------------------------------------------------
# service.py — _VaultCache (persistent, offline survival across reopen).
# ---------------------------------------------------------------------------


def _file_key_provider():
    from ggcommons.credentials.keyprovider import FileKeyProvider
    from ggcommons.credentials import crypto
    return FileKeyProvider(crypto.random(crypto.KEY_LEN))


class _SeededSource(ParameterSource):
    """A fake source yielding a secure + versioned value and a plain value."""

    def __init__(self):
        self.calls = 0

    def fetch(self, name):
        self.calls += 1
        if name == "/db/password":
            return ParamValue(b"hunter2", secure=True, version="5")
        if name == "/db/host":
            return ParamValue.plain(b"db.example.com")
        return None

    def fetch_by_path(self, path, recursive):
        return []

    def source_id(self):
        return "seeded"


def test_persistent_vault_cache_survives_reopen(tmp_path):
    from ggcommons.parameters.service import DefaultParameterService as DPS
    from ggcommons.credentials import LocalVault

    path = str(tmp_path / "param-cache")
    provider = _file_key_provider()
    vault = LocalVault.open(path, provider, 1)
    lock = threading.Lock()
    src = _SeededSource()
    svc = DPS.with_persistent_cache(
        src, vault, lock, ["/db/password", "/db/host"], []
    )
    svc.refresh()
    assert svc.get("/db/host") == "db.example.com"
    assert svc.get("/db/password") == "hunter2"
    # secure + version labels rode through into the persistent cache.
    assert svc.names("/db") == ["/db/host", "/db/password"]
    sub = svc.get_by_path("/db")
    assert sub["/db/host"] == "db.example.com"
    assert svc.stats().parameter_count == 2

    # Reopen with a brand-new vault handle + a source that now ERRORS (offline). The persisted
    # encrypted cache still serves both values across the "restart".
    provider2 = provider  # same KEK (file key persisted on disk by the first open)
    vault2 = LocalVault.open(path, provider2, 1)
    offline = _FailingSource()
    svc2 = DPS.with_persistent_cache(vault=vault2, lock=threading.Lock(),
                                     source=offline, sync_names=["/db/host"], sync_paths=[])
    # Bootstrap refresh fails but cache is non-empty -> non-fatal.
    svc2.refresh()
    assert svc2.get("/db/host") == "db.example.com"
    assert svc2.get("/db/password") == "hunter2"
    assert svc2.stats().refresh_failures == 1


def test_persistent_vault_cache_missing_name_returns_none(tmp_path):
    from ggcommons.parameters.service import DefaultParameterService as DPS
    from ggcommons.credentials import LocalVault

    path = str(tmp_path / "vc")
    vault = LocalVault.open(path, _file_key_provider(), 1)
    svc = DPS.with_persistent_cache(_SeededSource(), vault, threading.Lock(), [], [])
    # Nothing synced yet => a get on the persistent cache returns None (not an error).
    assert svc.get("/db/host") is None
    assert svc.get_bytes("/db/host") is None
    assert svc.names("/") == []
    assert svc.stats().parameter_count == 0


def test_refresher_loop_swallows_refresh_errors():
    # The background thread keeps running when a refresh raises (already counted in Inner.refresh).
    from ggcommons.parameters.service import _Inner, _Refresher, _MemoryCache

    inner = _Inner(_FailingSource(), _MemoryCache(), ["/x"], [])
    r = _Refresher(inner, 1)
    try:
        # Wait for at least one loop tick to exercise the try/except in _loop.
        deadline = time.time() + 5
        while time.time() < deadline and inner.failures == 0:
            time.sleep(0.1)
        assert inner.failures >= 1
    finally:
        r.close()


def test_open_from_config_persistent_cache_override(tmp_path, monkeypatch):
    # An env source with cache.persist=True forces the persistent VaultCache path through config.
    monkeypatch.setenv("GGTEST_PERSIST_MYAPP_REGION", "eu-west-1")
    cfg = {
        "source": {"type": "env", "prefix": "GGTEST_PERSIST_"},
        "refreshIntervalSecs": 0,
        "cache": {"persist": True, "path": str(tmp_path / "pcache")},
        "sync": {"names": ["/myapp/region"]},
    }
    s = open_from_config(cfg)
    assert s.get("/myapp/region") == "eu-west-1"
    assert s.stats().source == "env"


# ---------------------------------------------------------------------------
# config.py — _build_source branches, _lenient_int/_path_entries errors, bootstrap swallow.
# ---------------------------------------------------------------------------


def test_config_build_mounted_dir_source(tmp_path):
    (tmp_path / "region").write_bytes(b"ap-south-1")
    cfg = {
        "source": {"type": "mountedDir", "root": str(tmp_path), "securePaths": ["/secret"]},
        "refreshIntervalSecs": 0,
        "sync": {"paths": ["/"]},
    }
    s = open_from_config(cfg)
    assert s.get("/region") == "ap-south-1"
    assert s.stats().source == "mountedDir"


def test_config_mounted_dir_requires_root():
    cfg = {"source": {"type": "mountedDir"}, "refreshIntervalSecs": 0}
    with pytest.raises(ParameterError):
        open_from_config(cfg)


def test_config_unknown_source_raises():
    cfg = {"source": {"type": "bogus"}, "refreshIntervalSecs": 0}
    with pytest.raises(ParameterError):
        open_from_config(cfg)


def test_config_build_awssm_source_branch(monkeypatch):
    # Exercise the awsSsm branch in _build_source without constructing a real boto3 client.
    from ggcommons.parameters import ssm
    fake = _FakeSsmClient()
    monkeypatch.setattr(ssm.AwsSsmSource, "__init__",
                        lambda self, region=None, endpoint_url=None, with_decryption=True: (
                            setattr(self, "_with_decryption", with_decryption),
                            setattr(self, "_client", fake), None)[-1])
    from ggcommons.parameters.config import _build_source
    src = _build_source({"type": "awsSsm", "region": "us-east-1", "withDecryption": True})
    assert src.source_id() == "awsSsm"
    # A remote (awsSsm) source defaults to the persistent cache; with the fake client it resolves.
    cfg = {
        "source": {"type": "awsSsm"},
        "refreshIntervalSecs": 0,
        "cache": {"persist": False},  # avoid creating an encrypted vault file in this branch test
        "sync": {"names": ["/app/host"]},
    }
    s = open_from_config(cfg)
    assert s.get("/app/host") == "h"
    assert s.stats().source == "awsSsm"


def test_lenient_int_rejects_bool_and_string():
    from ggcommons.parameters.config import _lenient_int
    assert _lenient_int(None, 42) == 42
    with pytest.raises(ParameterError):
        _lenient_int(True, 0)
    with pytest.raises(ParameterError):
        _lenient_int("300", 0)


def test_path_entries_rejects_invalid_entry():
    from ggcommons.parameters.config import _path_entries
    assert _path_entries(None) == []
    assert _path_entries(["/a"]) == [("/a", True)]
    assert _path_entries([{"path": "/b", "recursive": False}]) == [("/b", False)]
    # Default-recursive when an object omits the flag.
    assert _path_entries([{"path": "/c"}]) == [("/c", True)]
    with pytest.raises(ParameterError):
        _path_entries([123])


def test_config_bootstrap_failure_is_swallowed():
    # An awsSsm source isn't reachable, but bootstrapOnStart failure must not raise out of
    # open_from_config (offline-first). Use cache.persist=False so no boto3 client is built? No —
    # awsSsm needs boto3; instead use a path source that fails via mountedDir on a missing root is
    # caught at build. Use env with a sync that simply has no values: bootstrap succeeds trivially.
    # To exercise the swallow path, monkeypatch refresh to raise.
    import ggcommons.parameters.config as cfgmod

    cfg = {
        "source": {"type": "env", "prefix": "GGTEST_BOOTFAIL_"},
        "refreshIntervalSecs": 0,
        "bootstrapOnStart": True,
        "sync": {"names": ["/x"]},
    }
    orig = DefaultParameterService.refresh

    def boom(self):
        raise ParameterError("bootstrap boom")

    cfgmod.DefaultParameterService.refresh = boom
    try:
        s = open_from_config(cfg)  # must not raise
    finally:
        cfgmod.DefaultParameterService.refresh = orig
    assert s.stats().source == "env"


# ---------------------------------------------------------------------------
# ssm.py — AwsSsmSource logic with a monkeypatched boto3 client (no live AWS, no moto).
# ---------------------------------------------------------------------------

boto3 = pytest.importorskip("boto3")


class _FakeParameterNotFound(Exception):
    pass


class _FakeSsmClient:
    """A stand-in SSM client covering get_parameter / get_parameters_by_path + pagination."""

    class exceptions:
        ParameterNotFound = _FakeParameterNotFound

    def __init__(self, **kwargs):
        self.kwargs = kwargs
        self.params = {
            "/app/host": {"Value": "h", "Type": "String", "Version": 3},
            "/app/secret": {"Value": "sshh", "Type": "SecureString", "Version": 1},
        }
        self.raise_on_get = False

    def get_parameter(self, Name, WithDecryption):
        if self.raise_on_get:
            raise RuntimeError("boom")
        if Name not in self.params:
            raise _FakeParameterNotFound()
        p = dict(self.params[Name])
        p["Name"] = Name
        return {"Parameter": p}

    def get_parameters_by_path(self, **kwargs):
        if getattr(self, "raise_on_path", False):
            raise RuntimeError("path boom")
        # Emulate two pages via NextToken.
        token = kwargs.get("NextToken")
        if token is None:
            p = dict(self.params["/app/host"])
            p["Name"] = "/app/host"
            return {"Parameters": [p], "NextToken": "page2"}
        p = dict(self.params["/app/secret"])
        p["Name"] = "/app/secret"
        return {"Parameters": [p]}


def _patched_ssm_source(monkeypatch, **kw):
    from ggcommons.parameters import ssm
    fake = _FakeSsmClient()
    monkeypatch.setattr(ssm.AwsSsmSource, "__init__",
                        lambda self, region=None, endpoint_url=None, with_decryption=True: (
                            setattr(self, "_with_decryption", with_decryption),
                            setattr(self, "_client", fake), None)[-1])
    return ssm.AwsSsmSource(**kw), fake


def test_ssm_fetch_string_and_secure(monkeypatch):
    src, _ = _patched_ssm_source(monkeypatch)
    plain = src.fetch("/app/host")
    assert plain.value == b"h"
    assert plain.secure is False
    assert plain.version == "3"
    sec = src.fetch("/app/secret")
    assert sec.secure is True
    assert sec.version == "1"
    assert src.source_id() == "awsSsm"


def test_ssm_fetch_not_found_returns_none(monkeypatch):
    src, _ = _patched_ssm_source(monkeypatch)
    assert src.fetch("/app/missing") is None


def test_ssm_fetch_error_wrapped(monkeypatch):
    src, fake = _patched_ssm_source(monkeypatch)
    fake.raise_on_get = True
    with pytest.raises(ParameterError):
        src.fetch("/app/host")


def test_ssm_fetch_by_path_paginates(monkeypatch):
    src, _ = _patched_ssm_source(monkeypatch)
    items = dict((n, v) for n, v in src.fetch_by_path("/app", recursive=True))
    assert set(items) == {"/app/host", "/app/secret"}
    assert items["/app/secret"].secure is True


def test_ssm_fetch_by_path_error_wrapped(monkeypatch):
    src, fake = _patched_ssm_source(monkeypatch)
    fake.raise_on_path = True
    with pytest.raises(ParameterError):
        src.fetch_by_path("/app", recursive=True)


def test_ssm_to_value_none_when_no_value():
    from ggcommons.parameters.ssm import AwsSsmSource
    assert AwsSsmSource._to_value({"Type": "String"}) is None


def test_ssm_missing_boto3_raises(monkeypatch):
    # Simulate boto3 not installed: AwsSsmSource construction must raise a clear ParameterError.
    import builtins
    from ggcommons.parameters.ssm import AwsSsmSource
    real_import = builtins.__import__

    def fake_import(name, *args, **kwargs):
        if name == "boto3":
            raise ImportError("no boto3")
        return real_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    with pytest.raises(ParameterError):
        AwsSsmSource()


# ---------------------------------------------------------------------------
# Schema-acceptance regression: a `parameters` section must pass config validation
# (mirroring how a `credentials` section is accepted at the permissive root).
# ---------------------------------------------------------------------------


def test_parameters_section_passes_config_validation():
    from ggcommons.validation.configuration_validator import ConfigurationValidator

    if not ConfigurationValidator.is_validation_available():
        pytest.skip("jsonschema/schema not available")

    config = {
        "component": {"global": {"name": "com.example.MyComp"}},
        "parameters": {
            "source": {"type": "env", "prefix": "GG_PARAM_"},
            "sync": {"names": ["/myapp/region"], "paths": ["/myapp"]},
            "refreshIntervalSecs": 300,
        },
    }
    # A parameters section is accepted (it's a known top-level section, validated permissively).
    ConfigurationValidator.validate(config)
