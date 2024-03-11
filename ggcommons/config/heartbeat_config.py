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
    DEFAULT_HEARTBEAT_MESSAGING_TOPIC = "ggcommons/{ThingName}/{ComponentName}/heartbeat"
    DEFAULT_HEARTBEAT_MESSAGING_DESTINATION = "ipc"
    __DEFAULT_HEARTBEAT_INTERVAL_SECS = 5
    __DEFAULT_HEARTBEAT_TARGETS = [
        {
            "type": "messaging",
            "config": {
                "destination": DEFAULT_HEARTBEAT_MESSAGING_DESTINATION,
                "topic": DEFAULT_HEARTBEAT_MESSAGING_TOPIC
            }
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
            if "intervalSecs" in heartbeat_json:
                self._interval_secs = heartbeat_json["intervalSecs"]
            if "measures" in heartbeat_json:
                if "cpu" in heartbeat_json["measures"]:
                    self._include_cpu = heartbeat_json["measures"]["cpu"]
                if "memory" in heartbeat_json["measures"]:
                    self._include_memory = heartbeat_json["measures"]["memory"]
                if "disk" in heartbeat_json["measures"]:
                    self._include_disk = heartbeat_json["measures"]["disk"]
                if "files" in heartbeat_json["measures"]:
                    self._include_files = heartbeat_json["measures"]["files"]
                if "threads" in heartbeat_json["measures"]:
                    self._include_threads = heartbeat_json["measures"]["threads"]
                if "fds" in heartbeat_json["measures"]:
                    self._include_fds = heartbeat_json["measures"]["fds"]
            if "targets" in heartbeat_json:
                self._targets = heartbeat_json["targets"]

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
                "fds": self._include_fds()
            },
            "targets": self._targets
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
