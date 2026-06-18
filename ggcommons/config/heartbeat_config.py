import json


# {
#     "intervalSecs": 5,
#     "measures": {
#         "cpu": true,
#         "memory": true
#         "disk": false
#     },
#     "targets": [
#         {
#             "type": "metric"
#         },
#         {
#             "type": "messaging",
#             "config": {
#                 "destination": "ipc",
#                 "topic": "{ThingName}/{ComponentName}/heartbeat"
#              }
#         }
#     ]
# }


class HeartbeatConfiguration:
    DEFAULT_HEARTBEAT_MESSAGING_TOPIC = (
        "ggcommons/{ThingName}/{ComponentName}/heartbeat"
    )
    DEFAULT_HEARTBEAT_MESSAGING_DESTINATION = "ipc"
    __DEFAULT_HEARTBEAT_INTERVAL_SECS = 5
    __DEFAULT_HEARTBEAT_TARGETS = [
        {
            "type": "messaging",
            "config": {
                "destination": DEFAULT_HEARTBEAT_MESSAGING_DESTINATION,
                "topic": DEFAULT_HEARTBEAT_MESSAGING_TOPIC,
            },
        }
    ]

    def __init__(self, heartbeat_json):
        self._interval_secs = HeartbeatConfiguration.__DEFAULT_HEARTBEAT_INTERVAL_SECS
        self._include_cpu = True
        self._include_memory = True
        self._include_disk = False
        self._include_files = False
        self._include_fds = False
        self._include_threads = False
        self._targets = self.__DEFAULT_HEARTBEAT_TARGETS

        if heartbeat_json is not None:
            self._interval_secs = heartbeat_json.get("intervalSecs", self._interval_secs)
            measures = heartbeat_json.get("measures", {})
            self._include_cpu = measures.get("cpu", self._include_cpu)
            self._include_memory = measures.get("memory", self._include_memory)
            self._include_disk = measures.get("disk", self._include_disk)
            self._include_files = measures.get("files", self._include_files)
            self._include_threads = measures.get("threads", self._include_threads)
            self._include_fds = measures.get("fds", self._include_fds)
            self._targets = heartbeat_json.get("targets", self._targets)

    def to_dict(self):
        dict_rep = {
            "topic": self._topic,
            "intervalSecs": self._interval_secs,
            "metric": {
                "cpu": self.include_cpu(),
                "memory": self.include_memory(),
                "disk": self.include_disk(),
                "files": self.include_files(),
                "threads": self.include_threads(),
                "fds": self._include_fds(),
            },
            "targets": self._targets,
        }
        return dict_rep

    def __str__(self):
        return json.dumps(self.to_dict(), indent=2)

    def get_interval_secs(self) -> int:
        return self._interval_secs

    def include_cpu(self) -> bool:
        return self._include_cpu

    def include_memory(self) -> bool:
        return self._include_memory

    def include_disk(self) -> bool:
        return self._include_disk

    def include_files(self) -> bool:
        return self._include_files

    def include_threads(self) -> bool:
        return self._include_threads

    def include_fds(self) -> bool:
        return self._include_fds

    def get_targets(self) -> list[dict]:
        return self._targets
