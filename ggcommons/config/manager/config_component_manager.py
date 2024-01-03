import json
import logging
from abc import ABC

from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message import MessageBuilder
from ggcommons.messaging.message import Message

logger = logging.getLogger("ConfigComponentManager")

class ConfigComponentManager(ConfigManager, ABC):
    _GET_TOPIC = "ggcommons/{}/config/get/{}"
    _UPDATED_TOPIC = "ggcommons/{}/config/{}/updated"
    _source = ""
    _DEFAULT_CONFIGURATION = {
        "logging": {},
        "source": {},
        "heartbeat": {},
        "component": {"global": {}, "instances": []},
    }

    def load_and_apply_config(self, topic: str, message: Message):
        logger.info("Updated config message received")
        config = message.get_body()
        self.configuration_changed(config)

    def __init__(self, component_name: str):
        super().__init__(component_name)
        self._GET_TOPIC = self._GET_TOPIC.format(self.get_thing_name(), component_name)
        self._UPDATED_TOPIC = self._UPDATED_TOPIC.format(self.get_thing_name(), component_name)
        self.init()
        MessagingClient.subscribe(self._UPDATED_TOPIC, self.load_and_apply_config)

    def _load_configuration(self) -> dict:
        request_payload = {}
        request = MessageBuilder.build_from_config("GetConfiguration", "1.0", request_payload, self)
        response = MessagingClient.request(self._GET_TOPIC, request).get(timeout=30)
        body = {}
        if response[0]:
            if type(response[1].body) == str:
                body = json.loads(response[1].body)
            elif type(response[1]) == Message:
                reply_message:Message = response[1]
                body = reply_message.body
        logger.debug("Fetched body of message as %s",body)
        return body

    def get_config_source(self) -> str:
        return f"Config Manager Component (source topic name: {self._GET_TOPIC})"
