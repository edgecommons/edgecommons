import json

# {
#     "intervalSecs": 5,
#     "topic": "heartbeat/{ThingName}/{ComponentName}",
#     "measures": {
#         "cpu": true,
#         "memory": true,
#         "disk": false,
#         "files": false,
#         "fds": false,
#         "threads": false
#     }
# }


class HeartbeatConfiguration:
    __DEFAULT_HEARTBEAT_TOPIC = "ggcommons/{ThingName}/{ComponentName}/heartbeat"
    __DEFAULT_HEARTBEAT_INTERVAL_SECS = 5

    def __init__(self, heartbeat_json):
        self._interval_secs = HeartbeatConfiguration.__DEFAULT_HEARTBEAT_INTERVAL_SECS
        self._include_cpu = True
        self._include_memory = True
        self._include_disk = False
        self._include_files = False
        self._include_fds = False
        self._include_threads = False

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

    def to_dict(self):
        dict_rep = {
            "intervalSecs": self._interval_secs,
            "measures": {
                "cpu": self.include_cpu(),
                "memory": self.include_memory(),
                "disk": self.include_disk(),
                "files": self.include_files(),
                "threads": self.include_threads(),
                "fds": self._include_fds()
            },
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

    def include_files(self):
        return self._include_files

    def include_threads(self):
        return self._include_threads

    def include_fds(self):
        return self._include_fds
