import json

class SourceConfiguration:

    def __init__(self, source_json: dict = None):
        self._hierarchy = source_json if source_json is not None else {}

    def to_dict(self):
        return self._hierarchy

    def __str__(self):
        return json.dumps(self.to_dict(), indent=2)

