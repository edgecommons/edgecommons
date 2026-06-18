import shutil
import psutil
import os
import platform
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ggcommons.config.manager.config_manager import ConfigManager


class HeartbeatMonitor:
    def __init__(self, config_service: "ConfigManager"):
        self._config_service = config_service
        self._config = config_service.get_heartbeat_config()
        self._pid = None
        self._proc_info = None
        self._platform = platform.system()
        self.pid, self.proc_info = HeartbeatMonitor.build_proc_info()

    def get_stats(self):
        data = {}
        cpu_data = self.cpu_usage()
        memory_data = self.memory_usage()
        disk_data = self.disk_usage()
        thread_data = self.thread_count()
        files_data = self.open_files()
        fds = self.file_descriptors()
        # Check for conflicting configurations
        if cpu_data is not None:
            data["cpu"] = cpu_data
        if memory_data is not None:
            data["memory"] = memory_data
        if disk_data is not None:
            data["disk"] = disk_data
        if thread_data is not None:
            data["threads"] = thread_data
        if files_data is not None:
            data["files"] = files_data
        if fds is not None:
            data["fds"] = fds
        return data

    @staticmethod
    def build_proc_info():
        pid = os.getpid()
        proc_info = psutil.Process(pid)
        return pid, proc_info

    def cpu_usage(self):
        cpu = None
        if self._config.include_cpu():
            cpu = {}
            usage = self.proc_info.cpu_percent()
            cpu["cpu_usage"] = usage
        return cpu

    def memory_usage(self):
        memory = None
        if self._config.include_memory():
            memory = {}
            usage = self.proc_info.memory_info().rss / 1000000
            memory["memory_usage"] = usage
        return memory

    def open_files(self):
        open_files = None
        if self._config.include_files():
            open_files = {}
            usage = len(self.proc_info.open_files())
            open_files["files"] = usage
        return open_files

    def file_descriptors(self):
        file_descriptors = None
        if self._config.include_fds():
            file_descriptors = {}
            if self._platform != "Windows":
                usage = self.proc_info.num_fds()
            else:
                usage = self.proc_info.num_handles()
            file_descriptors["fds"] = usage
        return file_descriptors

    def thread_count(self):
        thread_count = None
        if self._config.include_threads():
            thread_count = {}
            usage = len(self.proc_info.threads())
            thread_count["threads"] = usage
        return thread_count

    @staticmethod
    def __get_disk_usage():
        usage = shutil.disk_usage("..")
        usage = {"total": usage[0], "used": usage[1], "free": usage[2]}
        return usage

    def disk_usage(self):
        disk = None
        if self._config.include_disk():
            disk = {}
            disk_usage = HeartbeatMonitor.__get_disk_usage()
            total = disk_usage["total"] / 1000000000
            used = disk_usage["used"] / 1000000000
            free = disk_usage["free"] / 1000000000

            disk["disk_total"] = total
            disk["disk_used"] = used
            disk["disk_free"] = free
        return disk


if __name__ == "__main__":
    from ggcommons.config.manager.file_config_manager import FileConfigManager

    print(os.getcwd())
    config = FileConfigManager("PYTHON_TEST", "../../config_3.json")
    monitor = HeartbeatMonitor(config)
    print(monitor.pid)
    print(monitor.get_stats())
