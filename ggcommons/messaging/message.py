# (c) 2022 Amazon Web Services, Inc. or its affiliates. All Rights Reserved.
# This AWS Content is provided subject to the terms of the AWS Customer Agreement
# available at http://aws.amazon.com/agreement or other written agreement between
# Customer and Amazon Web Services, Inc.

import json
import logging
from uuid import uuid4
from typing import TYPE_CHECKING

from ggcommons.utils import Utils

if TYPE_CHECKING:
    from ggcommons.config.manager.config_manager import ConfigManager


logger = logging.getLogger("Message")


class MessageHeader:
    REPLY_MESSAGE_TOPIC_PREFIX = "ggcommons/reply-"

    def __init__(
        self,
        name: str,
        version: str,
        correlation_id: str = None,
        timestamp: str = None,
        uuid: str = None,
        reply_to: str = None,
    ):
        self.name = name
        self.version = version
        self.timestamp = timestamp if timestamp is not None else Utils.get_utc_z()
        self.correlation_id = correlation_id if correlation_id is not None else str(uuid4())
        self.uuid = uuid if uuid is not None else str(uuid4())
        self.reply_to = reply_to

    @staticmethod
    def from_dict(src: dict):
        name = src.get("name")
        version = src.get("version")
        timestamp = src.get("timestamp")
        uuid = src.get("uuid")
        correlation_id = src.get("correlation_id")
        if 'reply_to' in src:
            reply_to = src.get("reply_to")
        return MessageHeader(name, version, correlation_id, timestamp, uuid, reply_to)

    def to_dict(self) -> dict:
        header = {
            "name": self.name,
            "version": self.version,
            "timestamp": self.timestamp,
            "uuid": self.uuid,
            "correlation_id": self.correlation_id,
        }
        if self.reply_to is not None:
            header["reply_to"] = self.reply_to
        return header

    def make_request(self, reply_to: str = None) -> str:
        if reply_to is None:
            reply_to = self.REPLY_MESSAGE_TOPIC_PREFIX + str(uuid4())
        self.reply_to = reply_to
        logger.debug(f"Setting replyTo field as {self.reply_to}")
        return self.reply_to

    def get_reply_to(self) -> str:
        return self.reply_to

    def set_correlation_id(self, correlation_id: str):
        self.correlation_id = correlation_id


class MessageTags:
    def __init__(self, thing_name: str, tags: dict = None):
        self.thing_name = thing_name
        self.tags = tags or {}

    @staticmethod
    def from_config(config_service: 'ConfigManager'):
        tag_config = config_service.get_tag_config()
        if tag_config is not None:
            return MessageTags(config_service.get_thing_name(), tag_config.to_dict())
        else:
            return MessageTags(config_service.get_thing_name(), {})

    @staticmethod
    def from_dict(src: dict):
        thing = src.get("thing")
        tags_dict = {k: v for k, v in src.items() if k != "thing"}
        return MessageTags(thing, tags_dict)

    def inject_tag(self, key: str, value: str):
        self.tags[key] = value

    def to_dict(self) -> dict:
        result = dict(self.tags)
        # Omit the "thing" key entirely when there is no thing name (rather than
        # emitting "thing": null), matching the Java/Rust serialization.
        if self.thing_name is not None:
            result["thing"] = self.thing_name
        return result


class Message:
    def __init__(self):
        self.header = None
        self.tags = None
        self.body = None
        self.raw = None

    def to_dict(self) -> dict:
        if self.raw is None:
            msg = {}
            if self.header is not None:
                msg["header"] = self.header.to_dict()
            if self.tags is not None:
                msg["tags"] = self.tags.to_dict()
            msg["body"] = self.body
            return msg
        else:
            return {"raw": self.raw}

    def __str__(self) -> str:
        return json.dumps(self.to_dict())

    def dumps(self, indent=None) -> str:
        msg = {}
        if self.header is not None:
            msg["header"] = self.header.to_dict()
        if self.tags is not None:
            msg["tags"] = self.tags.to_dict()
        msg["body"] = self.body
        return json.dumps(msg, indent=indent)

    def get_correlation_id(self) -> str:
        if self.header is None:
            return None
        return self.header.correlation_id

    def get_header(self) -> MessageHeader:
        return self.header

    def get_tags(self) -> MessageTags:
        return self.tags

    def get_source(self):
        """Backward compatibility alias for get_tags()"""
        return self.get_tags()

    def inject_tag(self, key: str, value: str):
        if self.tags is None:
            self.tags = MessageTags(None)
        self.tags.inject_tag(key, value)

    def get_body(self):
        return self.body

    def get_payload(self):
        """Backward compatibility alias for get_body()"""
        return self.get_body()

    def get_raw(self):
        return self.raw

    def make_request(self, reply_to: str = None) -> str:
        if self.header is None:
            self.header = MessageHeader("None", "None")
            logger.warning("Attempting to make request from message with no header")
        return self.header.make_request(reply_to)

    def set_correlation_id(self, correlation_id: str):
        if self.header is None:
            self.header = MessageHeader("None", "None", correlation_id)
        else:
            self.header.set_correlation_id(correlation_id)

    @staticmethod
    def from_object(msg_contents):
        message = Message()
        logger.debug("In Message.from_object")
        
        if isinstance(msg_contents, dict):
            logger.debug(f"Message contents: {msg_contents}")
            if "header" in msg_contents:
                logger.debug("processing header")
                message.header = MessageHeader.from_dict(msg_contents["header"])
                logger.debug("header deserialized")
            if "tags" in msg_contents:
                logger.debug("processing tags")
                message.tags = MessageTags.from_dict(msg_contents["tags"])
                logger.debug("tags deserialized")
            if "body" in msg_contents:
                logger.debug("processing body")
                message.body = msg_contents["body"]
                logger.debug("body deserialized")
            if not any(key in msg_contents for key in ["header", "tags", "body"]):
                logger.debug("Dict contained raw data: Assigning to raw")
                message.raw = msg_contents
        else:
            logger.debug("Message not instance of dict, assigning to raw")
            message.raw = msg_contents
            
        return message