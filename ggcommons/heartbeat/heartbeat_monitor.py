import shutil
import psutil
import os
from ggcommons.config.heartbeat_config import HeartbeatConfiguration


class HeartbeatMonitor:
    def __init__(self, hb_config: HeartbeatConfiguration):
        self._config = hb_config
        self._pid = None
        self._proc_info = None
        self.pid, self.proc_info = HeartbeatMonitor.build_proc_info()

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
            cpu["cpu_usage(%)"] = usage
        return cpu

    def memory_usage(self):
        memory = None
        if self._config.include_memory():
            memory = {}
            usage = self.proc_info.memory_info().rss / 1000000
            memory["memory_usage(MB)"] = usage
        return memory

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

            disk["total(GB)"] = total
            disk["used(GB)"] = used
            disk["free(GB)"] = free
        return disk


# if __name__ == "__main__":
#     import argparse
#     from ggcommons.config.config_manager import ConfigManagerBuilder
#
#     parser = argparse.ArgumentParser(description="Testing config_manager")
#     parser.add_argument('--configFile', default='config/samples/config_1.json', required=False)
#
#     config = ConfigManagerBuilder.build(parser.parse_args(), "HeartbeatMonitorTester")
#     monitor = HeartbeatMonitor(config.get_heartbeat_config())
#     print(monitor.pid)
#     print(monitor.disk_usage())
#     print(monitor.cpu_usage())
#     print(monitor.memory_usage())
