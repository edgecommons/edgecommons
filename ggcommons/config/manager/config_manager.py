import abc
import logging
import os
import sys
import time
from abc import abstractmethod

from ggcommons.config.heartbeat_config import HeartbeatConfiguration
from ggcommons.config.metric_config import MetricConfiguration
from ggcommons.config.tag_config import TagConfiguration
from ggcommons.config.logging_config import LoggingConfiguration

# import unittest

logger = logging.getLogger("ConfigManager")


class ConfigManager(metaclass=abc.ABCMeta):
    def __init__(self, component_name: str):
        self._tag_config = None
        self._heartbeat_config = None
        self._metric_config = None
        self._component_config = None
        self._global_config = {}
        self._instances = {}
        self._change_listeners = []
        self._thing_name = (
            "NOT_GREENGRASS"
            if "AWS_IOT_THING_NAME" not in os.environ
            else os.environ["AWS_IOT_THING_NAME"]
        )
        self._component_name = component_name

    def init(self):
        config = self._load_configuration()
        if config is None:
            config = {"component": {}}
        self._apply_config(config)

    def _apply_config(self, config: dict):
        logging_json = None if "logging" not in config else config["logging"]
        self._logging_config = LoggingConfiguration(logging_json)
        logging.basicConfig(
            format=self._logging_config.get_format(),
            level=self._logging_config.get_level(),
        )
        logging.Formatter.converter = time.gmtime
        logging.StreamHandler(sys.stdout)

        tag_json = None if "tags" not in config else config["tags"]
        self._tag_config = TagConfiguration(tag_json)

        heartbeat_json = None if "heartbeat" not in config else config["heartbeat"]
        self._heartbeat_config = HeartbeatConfiguration(heartbeat_json)

        metric_json = None if "metricEmission" not in config else config["metricEmission"]
        self._metric_config = MetricConfiguration(metric_json)

        component_json = (
            {"global": {}, "instances": []}
            if "component" not in config
            else config["component"]
        )
        self._component_config = component_json
        self._global_config = (
            {}
            if "global" not in self._component_config
            else self._component_config["global"]
        )
        self._gen_instances_map()

    def _gen_instances_map(self):
        if "instances" in self._component_config:
            for instance in self._component_config["instances"]:
                self._instances[instance["id"]] = instance
                logger.debug(f"loaded config for {self._instances[instance['id']]}")

    def configuration_changed(self, new_config: dict) -> bool:
        logger.debug(f"configuration_changed: Applying new config: {new_config}")
        self._apply_config(new_config)

        logger.info(f"configuration_changed: Notifying change listeners")
        for listener in self._change_listeners:
            listener.on_configuration_change(new_config)
        return True

    def resolve_template(self, template: str) -> str:
        ret_val = template
        if "{ThingName}" in template:
            ret_val = ret_val.replace("{ThingName}", self._thing_name)
        if "{ComponentName}" in template:
            ret_val = ret_val.replace("{ComponentName}", self._component_name)
        tag_dict = (
            {} if self._tag_config is None else self._tag_config.to_dict()
        )
        for k in tag_dict.keys():
            key_template = "{" + k + "}"
            if key_template in template:
                ret_val = ret_val.replace(key_template, tag_dict[k])
        return ret_val

    @abstractmethod
    def _load_configuration(self) -> dict:
        pass

    def get_global_config(self) -> dict:
        return self._global_config

    def get_instance_ids(self) -> list:
        return [*self._instances]

    def get_instance_config(self, inst_id) -> dict:
        return self._instances[inst_id]

    def get_tag_config(self) -> TagConfiguration:
        return self._tag_config

    def get_heartbeat_config(self) -> HeartbeatConfiguration:
        return self._heartbeat_config

    def get_metric_config(self) -> MetricConfiguration:
        return self._metric_config

    def get_logging_config(self) -> LoggingConfiguration:
        return self._logging_config

    def get_thing_name(self) -> str:
        return self._thing_name

    def get_component_name(self) -> str:
        return self._component_name

    def add_config_change_listener(self, listener):
        self._change_listeners.append(listener)

    @abstractmethod
    def get_config_source(self) -> str:
        pass
