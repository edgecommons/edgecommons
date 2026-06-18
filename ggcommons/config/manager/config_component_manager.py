import json
import logging


from ggcommons.messaging.messaging_client import MessagingClient
from ggcommons.config.manager.config_manager import ConfigManager
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.messaging.message import Message

logger = logging.getLogger("ConfigComponentManager")


class ConfigComponentManager(ConfigManager):
    _GET_TOPIC = "ggcommons/{}/config/get/{}"
    _UPDATED_TOPIC = "ggcommons/{}/config/{}/updated"

    def load_and_apply_config(self, topic: str, message: Message):
        logger.info("Updated config message received")
        config = message.get_body()
        self.configuration_changed(config)

    def __init__(self, thing_name: str, component_name: str):
        super().__init__(component_name, thing_name)
        self._GET_TOPIC = self._GET_TOPIC.format(self.get_thing_name(), component_name)
        self._UPDATED_TOPIC = self._UPDATED_TOPIC.format(
            self.get_thing_name(), component_name
        )
        self._config_source = (
            f"Config Manager Component (source topic name: {self._GET_TOPIC})"
        )
        self.init()
        MessagingClient.subscribe(self._UPDATED_TOPIC, self.load_and_apply_config)

    def _load_configuration(self) -> dict:
        request_payload = {}
        request = MessageBuilder.create("GetConfiguration", "1.0") \
            .with_payload(request_payload) \
            .with_config(self) \
            .build()
        response = MessagingClient.request(self._GET_TOPIC, request).get(timeout=30)
        body = {}
        if response[0]:
            if isinstance(response[1].body, str):
                body = json.loads(response[1].body)
            elif isinstance(response[1], Message):
                reply_message: Message = response[1]
                body = reply_message.body
        logger.debug("Fetched body of message as %s", body)
        return body
