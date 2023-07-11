import json

# {
#     "intervalSecs": 5,
#     "metric": {
#         "cpu": true,
#         "memory": true
#         "disk": false
#     }
# }


class HeartbeatConfiguration:

    __DEFAULT_HEARTBEAT_TOPIC = 'heartbeat/{ThingName}/{ComponentName}'
    __DEFAULT_HEARTBEAT_INTERVAL_SECS = 5

    def __init__(self, heartbeat_json):
        self._topic = HeartbeatConfiguration.__DEFAULT_HEARTBEAT_TOPIC
        self._interval_secs = HeartbeatConfiguration.__DEFAULT_HEARTBEAT_INTERVAL_SECS
        self._include_cpu = True
        self._include_memory = True
        self._include_disk = False

        if heartbeat_json is not None:
            if 'intervalSecs' in heartbeat_json:
                self._interval_secs = heartbeat_json['intervalSecs']
            if 'topic' in heartbeat_json:
                self._topic = heartbeat_json['topic']
            if 'metric' in heartbeat_json:
                if 'cpu' in heartbeat_json['metric']:
                    self._include_cpu = heartbeat_json['metric']['cpu']
                if 'memory' in heartbeat_json['metric']:
                    self._include_memory = heartbeat_json['metric']['memory']
                if 'disk' in heartbeat_json['metric']:
                    self._include_disk = heartbeat_json['metric']['disk']

    def to_dict(self):
        dict_rep = {
            'topic': self._topic,
            'intervalSecs': self._interval_secs,
            'metric': {
                'cpu': self.include_cpu(),
                'memory': self.include_memory(),
                'disk': self.include_disk()
            }
        }
        return dict_rep

    def __str__(self):
        return json.dumps(self.to_dict(), indent=2)

    def get_topic(self) -> str:
        return self._topic

    def get_interval_secs(self) -> int:
        return self._interval_secs

    def include_cpu(self) -> bool:
        return self._include_cpu

    def include_memory(self) -> bool:
        return self._include_memory

    def include_disk(self) -> bool:
        return self._include_disk
