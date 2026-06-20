"""
Builder for creating Message instances with fluent API.
"""
import json
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from ggcommons.config.manager.config_manager import ConfigManager


class MessageBuilder:
    def __init__(self, name: str, version: str):
        self.name = name
        self.version = version
        self.correlation_id = None
        self.uuid = None
        self.timestamp = None
        self.reply_to = None
        self.payload = None
        self.config_service = None
        self.tags = None

    @staticmethod
    def create(name: str, version: str) -> 'MessageBuilder':
        return MessageBuilder(name, version)

    @staticmethod
    def from_object(msg_contents) -> 'MessageBuilder':
        if isinstance(msg_contents, dict) and "header" in msg_contents:
            header = msg_contents["header"]
            name = header.get("name", "unknown")
            version = header.get("version", "1.0")
            
            builder = MessageBuilder(name, version)
            
            if "correlation_id" in header:
                builder.with_correlation_id(header["correlation_id"])
            if "uuid" in header:
                builder.with_uuid(header["uuid"])
            if "timestamp" in header:
                builder.with_timestamp(header["timestamp"])
            if "reply_to" in header:
                builder.with_reply_to(header["reply_to"])
            if "body" in msg_contents:
                builder.with_payload(msg_contents["body"])
            if "tags" in msg_contents:
                builder.with_tags(msg_contents["tags"])
            
            return builder
        else:
            # Raw message without header structure
            builder = MessageBuilder("raw", "1.0")
            builder.with_payload(msg_contents)
            return builder

    def with_correlation_id(self, correlation_id: str) -> 'MessageBuilder':
        self.correlation_id = correlation_id
        return self

    def with_payload(self, payload) -> 'MessageBuilder':
        self.payload = payload
        return self

    def with_config(self, config_service: 'ConfigManager') -> 'MessageBuilder':
        self.config_service = config_service
        return self

    def with_tags(self, tags: dict) -> 'MessageBuilder':
        self.tags = tags
        return self

    def with_uuid(self, uuid: str) -> 'MessageBuilder':
        self.uuid = uuid
        return self

    def with_timestamp(self, timestamp: str) -> 'MessageBuilder':
        self.timestamp = timestamp
        return self

    def with_reply_to(self, reply_to: str) -> 'MessageBuilder':
        self.reply_to = reply_to
        return self

    def build(self):
        if self.config_service is None and self.tags is None:
            raise ValueError("Configuration service is required - call with_config()")

        from ggcommons.messaging.message import Message, MessageHeader, MessageTags

        message = Message()
        message.header = MessageHeader(self.name, self.version, self.correlation_id, self.timestamp, self.uuid, self.reply_to)
        
        if self.tags is not None:
            message.tags = MessageTags.from_dict(self.tags)
        elif self.config_service is not None:
            message.tags = MessageTags.from_config(self.config_service)
        
        if isinstance(self.payload, str):
            try:
                message.body = json.loads(self.payload)
            except json.JSONDecodeError:
                message.body = self.payload
        else:
            message.body = self.payload

        return message