import json
import copy


class TagConfiguration:
    def __init__(self, tag_json: dict = None):
        self._tags = copy.deepcopy(tag_json) if tag_json is not None else {}

    def to_dict(self):
        return copy.deepcopy(self._tags)

    def __str__(self):
        return json.dumps(self.to_dict(), indent=2)
