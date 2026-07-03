import json


# {
#     "enabled": true,
#     "intervalSecs": 5,
#     "measures": {
#         "cpu": true,
#         "memory": true,
#         "disk": false
#     },
#     "destination": "local"
# }


class HeartbeatConfiguration:
    """Configuration model for the component heartbeat (UNS-CANONICAL-DESIGN §4.3,
    D-U14/D-U20).

    The heartbeat is a library-owned UNS ``state`` keepalive published each tick to
    ``ecv1/{device}/{component}/main/state`` (body
    ``{"status":"RUNNING","uptimeSecs":n}``, best-effort ``{"status":"STOPPED"}`` on
    graceful shutdown), with the enabled system measures emitted as the metric ``sys``
    through the normal metric subsystem. The legacy ``targets[]`` array (the heartbeat
    topic-override drift knobs) is removed — hard cut; :meth:`get_destination` governs
    only the state keepalive's transport (``local`` vs ``iotcore``); the measures route
    through the metric subsystem's own target. Defaults: on / 5 s / local (M11)."""

    #: The schema default for ``heartbeat.destination`` — the local/IPC transport.
    DEFAULT_DESTINATION = "local"
    __DEFAULT_HEARTBEAT_INTERVAL_SECS = 5

    def __init__(self, heartbeat_json):
        self._enabled = True
        self._interval_secs = HeartbeatConfiguration.__DEFAULT_HEARTBEAT_INTERVAL_SECS
        self._include_cpu = True
        self._include_memory = True
        self._include_disk = False
        self._include_files = False
        self._include_fds = False
        self._include_threads = False
        self._destination = HeartbeatConfiguration.DEFAULT_DESTINATION

        if heartbeat_json is not None:
            self._enabled = heartbeat_json.get("enabled", self._enabled)
            self._interval_secs = heartbeat_json.get("intervalSecs", self._interval_secs)
            if not isinstance(self._interval_secs, (int, float)) or self._interval_secs < 1:
                self._interval_secs = HeartbeatConfiguration.__DEFAULT_HEARTBEAT_INTERVAL_SECS
            measures = heartbeat_json.get("measures", {})
            self._include_cpu = measures.get("cpu", self._include_cpu)
            self._include_memory = measures.get("memory", self._include_memory)
            self._include_disk = measures.get("disk", self._include_disk)
            self._include_files = measures.get("files", self._include_files)
            self._include_threads = measures.get("threads", self._include_threads)
            self._include_fds = measures.get("fds", self._include_fds)
            self._destination = heartbeat_json.get("destination", self._destination)

    def to_dict(self):
        # Mirrors the parsed input structure so the config round-trips:
        # {enabled, intervalSecs, measures: {...}, destination}.
        return {
            "enabled": self._enabled,
            "intervalSecs": self._interval_secs,
            "measures": {
                "cpu": self.include_cpu(),
                "memory": self.include_memory(),
                "disk": self.include_disk(),
                "files": self.include_files(),
                "threads": self.include_threads(),
                "fds": self.include_fds(),
            },
            "destination": self._destination,
        }

    def __str__(self):
        return json.dumps(self.to_dict(), indent=2)

    def is_enabled(self) -> bool:
        """Whether the heartbeat (state keepalive + ``sys`` measures metric) runs.
        Default ``True`` — on / 5 s / local (D-U14)."""
        return self._enabled

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

    def get_destination(self) -> str:
        """The publish destination of the ``state`` keepalive only — ``"local"`` (the
        local/IPC transport, the default) or ``"iotcore"`` (AWS IoT Core). The measures
        route through the metric subsystem's own target and are unaffected."""
        return self._destination
