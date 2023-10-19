import json
import logging


# {
#     "level": "INFO",
#     "foramt": "%(asctime)s %(threadName)s %(levelname)s %(filename)s %(funcName)s(%(lineno)d): %(message)s"
# }


class LoggingConfiguration:
    _default_level = logging.INFO
    _default_format = "%(asctime)s - %(levelname)s - %(module)s - %(funcName)s(%(lineno)d): %(message)s"

    def __init__(self, logging_json):
        self._level = self._default_level
        if logging_json is not None and "level" in logging_json:
            self._level = logging.getLevelName(logging_json["level"])

        self._format = self._default_format
        if logging_json is not None and "format" in logging_json:
            self._format = logging_json["format"]

    def to_dict(self):
        dict_rep = {
            "level": logging.getLevelName(self.get_level()),
            "format": self.get_format(),
        }
        return dict_rep

    def __str__(self):
        return json.dumps(self.to_dict(), indent=2)

    def get_level(self) -> int:
        return self._level

    def get_format(self) -> str:
        return self._format
