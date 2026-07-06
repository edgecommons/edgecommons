"""Unit tests for HeartbeatMonitor (psutil-backed system metrics).

The current process is a real process, so most reads work directly; values that are
platform-specific or environment-dependent are asserted structurally, not numerically.
"""
from types import SimpleNamespace

import pytest

from edgecommons.heartbeat.heartbeat_monitor import HeartbeatMonitor
from edgecommons.config.heartbeat_config import HeartbeatConfiguration


class FakeConfigService:
    def __init__(self, hb_config):
        self._hb = hb_config

    def get_heartbeat_config(self):
        return self._hb


def _monitor(measures):
    cfg = HeartbeatConfiguration({"measures": measures})
    return HeartbeatMonitor(FakeConfigService(cfg))


class TestHeartbeatMonitor:
    def test_all_disabled_returns_empty(self):
        m = _monitor({"cpu": False, "memory": False, "disk": False,
                      "files": False, "threads": False, "fds": False})
        assert m.get_stats() == {}

    def test_cpu_enabled(self):
        m = _monitor({"cpu": True, "memory": False})
        cpu = m.cpu_usage()
        assert cpu is not None and "cpu_usage" in cpu

    def test_cpu_disabled_returns_none(self):
        m = _monitor({"cpu": False})
        assert m.cpu_usage() is None

    def test_memory_enabled(self):
        m = _monitor({"memory": True})
        mem = m.memory_usage()
        assert mem is not None and mem["memory_usage"] > 0

    def test_threads_enabled(self):
        m = _monitor({"threads": True})
        th = m.thread_count()
        assert th is not None and th["threads"] >= 1

    def test_disk_enabled(self):
        m = _monitor({"disk": True})
        disk = m.disk_usage()
        assert disk is not None
        assert disk["disk_total"] > 0
        assert disk["disk_used"] >= 0
        assert disk["disk_free"] >= 0

    def test_disk_disabled(self):
        m = _monitor({"disk": False})
        assert m.disk_usage() is None

    def test_files_enabled(self):
        m = _monitor({"files": True})
        files = m.open_files()
        assert files is not None and "files" in files

    def test_file_descriptors_enabled(self):
        m = _monitor({"fds": True})
        fds = m.file_descriptors()
        assert fds is not None and "fds" in fds

    def test_fds_disabled(self):
        m = _monitor({"fds": False})
        assert m.file_descriptors() is None

    def test_get_stats_includes_enabled_only(self):
        m = _monitor({"cpu": True, "memory": True, "disk": False,
                      "files": False, "threads": True, "fds": False})
        stats = m.get_stats()
        assert set(stats.keys()) == {"cpu", "memory", "threads"}

    def test_file_descriptors_windows_branch(self, monkeypatch):
        # Force the non-Windows num_fds branch deterministically by faking proc_info.
        m = _monitor({"fds": True})
        m._platform = "Linux"
        m.proc_info = SimpleNamespace(num_fds=lambda: 7, num_handles=lambda: 99)
        assert m.file_descriptors()["fds"] == 7

    def test_file_descriptors_windows_uses_handles(self):
        m = _monitor({"fds": True})
        m._platform = "Windows"
        m.proc_info = SimpleNamespace(num_fds=lambda: 7, num_handles=lambda: 99)
        assert m.file_descriptors()["fds"] == 99

    def test_build_proc_info(self):
        pid, proc = HeartbeatMonitor.build_proc_info()
        assert isinstance(pid, int) and proc is not None
