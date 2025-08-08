import json
import logging
import os


class MetricConfiguration:
    # Default configuration values
    DEFAULT_MESSAGING_TOPIC = "{ThingName}/{ComponentName}/metric"
    DEFAULT_CLOUDWATCH_COMPONENT_TOPIC = "cloudwatch/metric/put"
    DEFAULT_TARGET = "log"
    DEFAULT_METRIC_NAMESPACE = "ggcommons"
    DEFAULT_METRIC_FILE_NAME_TEMPLATE = (
        "/greengrass/v2/logs/{ComponentFullName}_metric.log"
    )
    DEFAULT_INTERVAL_SECS = 5
    DEFAULT_MESSAGING_DESTINATION = "ipc"
    DEFAULT_LARGE_FLEET_WORKAROUND = False

    def __init__(self, json_config=None):
        self.logger = logging.getLogger(self.__class__.__name__)

        # Default values
        self._target = self.DEFAULT_TARGET
        self._namespace = self.DEFAULT_METRIC_NAMESPACE
        self._log_file_name_template = self.DEFAULT_METRIC_FILE_NAME_TEMPLATE
        self._topic = self.DEFAULT_MESSAGING_TOPIC
        self._interval_secs = self.DEFAULT_INTERVAL_SECS
        self._destination = self.DEFAULT_MESSAGING_DESTINATION
        self._large_fleet_workaround = self.DEFAULT_LARGE_FLEET_WORKAROUND

        if json_config:
            self._target = json_config.get("target", self._target)
            self._namespace = json_config.get("namespace", self._namespace)
            self._large_fleet_workaround = json_config.get(
                "largeFleetWorkaround", self._large_fleet_workaround
            )

            if self._target.lower() == "log":
                self._log_file_name_template = self.DEFAULT_METRIC_FILE_NAME_TEMPLATE
                if "targetConfig" in json_config:
                    if "logFileName" in json_config["targetConfig"]:
                        self._log_file_name_template = json_config["targetConfig"][
                            "logFileName"
                        ]

            if self._target.lower() == "messaging":
                self._topic = self.DEFAULT_MESSAGING_TOPIC
                target_config = json_config.get("targetConfig", {})
                self._topic = target_config.get("topic", self._topic)
                self._destination = target_config.get("destination", self._destination)

            if self._target.lower() == "cloudwatchcomponent":
                self._topic = self.DEFAULT_CLOUDWATCH_COMPONENT_TOPIC
                target_config = json_config.get("targetConfig", {})
                self._topic = target_config.get("topic", self._topic)

            if self._target.lower() == "cloudwatch":
                target_config = json_config.get("targetConfig", {})
                self._interval_secs = int(
                    target_config.get("intervalSecs", self._interval_secs)
                )
                if self._interval_secs < 1:
                    self._interval_secs = self.DEFAULT_INTERVAL_SECS

            self.logger.debug(
                f"Metric configuration: target={self._target}, namespace={self._namespace}, logFileName={self._log_file_name_template}, topic={self._topic}, intervalSecs={self._interval_secs}"
            )

    def to_dict(self):
        config = {"target": self._target, "targetConfig": {}}

        if self._target == "messaging":
            config["targetConfig"] = {
                "topic": self._topic,
                "destination": self._destination,
            }
        elif self._target == "cloudwatchcomponent":
            config["targetConfig"] = {"topic": self._topic}
        elif self._target == "cloudwatch":
            config["targetConfig"] = {"intervalSecs": self._interval_secs}
        elif self._target == "log":
            config["targetConfig"] = {"filename": self._log_file_name_template}

        return config

    def __str__(self):
        return json.dumps(self.to_dict())

    def get_target(self) -> str:
        return self._target

    def get_namespace(self) -> str:
        return self._namespace

    def get_log_file_name_template(self) -> str:
        return self._log_file_name_template

    def get_topic(self) -> str:
        return self._topic

    def get_interval_secs(self) -> int:
        return self._interval_secs

    def get_destination(self) -> str:
        return self._destination

    def get_large_fleet_workaround(self) -> bool:
        return self._large_fleet_workaround
