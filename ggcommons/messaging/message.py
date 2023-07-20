# (c) 2022 Amazon Web Services, Inc. or its affiliates. All Rights Reserved.
# This AWS Content is provided subject to the terms of the AWS Customer Agreement
# available at http://aws.amazon.com/agreement or other written agreement between
# Customer and Amazon Web Services, Inc.

import json
import logging
from uuid import uuid4
from datetime import datetime
from ggcommons.config.manager.config_manager import ConfigManager


logger = logging.getLogger("Message")


class MessageHeader:

    def __init__(self, name: str, version: str,
                 timestamp: str = None, correlation_id: str = None, uuid: str = None, reply_to: str = None):
        self.name = name
        self.version = version
        if timestamp is None:
            timestamp = datetime.now().isoformat()
        self.timestamp = timestamp
        if uuid is None:
            uuid = str(uuid4())
        self.uuid = uuid
        if correlation_id is None:
            correlation_id = str(uuid4())
        self.correlation_id = correlation_id
        self.reply_to = reply_to

    @staticmethod
    def from_dict(src: dict):
        name = src['name']
        version = src['version']
        timestamp = src['timestamp']
        uuid = src['uuid']
        correlation_id = src['correlation_id']
        reply_to = None
        if 'reply_to' in src:
            reply_to = src['reply_to']
        return MessageHeader(name, version,
                             timestamp=timestamp, correlation_id=correlation_id, uuid=uuid, reply_to=reply_to)

    def to_dict(self) -> dict:
        header = {
            'name': self.name,
            'version': self.version,
            'timestamp': self.timestamp,
            'uuid': self.uuid,
            'correlation_id': self.correlation_id
        }

        if self.reply_to is not None:
            header['reply_to'] = self.reply_to

        return header

    def dumps(self, indent=None) -> str:
        return json.dumps(self.to_dict(), indent=indent)

    def make_request(self, reply_to=None) -> str:
        if reply_to is None:
            reply_to = str(uuid4())
        self.reply_to = reply_to
        return self.reply_to

    def get_reply_to(self) -> str:
        return self.reply_to


class MessageSource:
    def __init__(self, thing_name: str, hierarchy: dict):
        self._thing_name = thing_name
        self._hierarchy = hierarchy

    @staticmethod
    def from_config(config_manager: ConfigManager):
        source_config = config_manager.get_source_config()
        if source_config is not None:
            thing_name = config_manager.get_thing_name()
            return MessageSource(thing_name, source_config.to_dict())
        else:
            return None

    @staticmethod
    def from_dict(src: dict):
        thing = src['thing']
        return MessageSource(thing, src)

    def to_dict(self) -> dict:
        source = {
            'thing': self._thing_name
        }
        for key, value in self._hierarchy.items():
            source[key] = value
        return source

    def dumps(self, indent=None) -> str:
        return json.dumps(self.to_dict(), indent=indent)


class Message:

    def __init__(self):
        self.header = None
        self.body = None
        self.source = None
        # if a message was not constructed with ggcommons, its contents will go into raw
        self.raw = None

    def to_dict(self) -> dict:
        if self.raw is None:
            msg = {'header': self.header.to_dict()}
            if self.source is not None:
                msg['source'] = self.source.to_dict()
            msg['body'] = self.body
            return msg
        else:
            return self.raw

    def dumps(self, indent=None) -> str:
        return json.dumps(self.to_dict(), indent=indent)

    def get_correlation_id(self) -> str:
        return self.header.correlation_id

    def get_header(self) -> MessageHeader:
        return self.header

    def get_source(self) -> MessageSource:
        return self.source

    def get_body(self):
        return self.body

    def get_raw(self):
        return self.raw

    def make_request(self, reply_to: str = None) -> str:
        if self.header is None:
            self.header = MessageHeader("None", "None", "0.1")
            logger.warning(f"Attempting to make request from message with no header")
        return self.header.make_request(reply_to)

    def set_correlation_id(self, correlation_id: str):
        self.get_header().correlation_id = correlation_id


class MessageBuilder:

    @staticmethod
    def build_from_config(name: str, version: str, payload,
                          config_manager: ConfigManager, correlation_id: str = None) -> Message:
        msg = Message()
        msg.header = MessageHeader(name, version, correlation_id=correlation_id)
        msg.source = MessageSource.from_config(config_manager)
        if isinstance(payload, str):
            try:
                body_dict = json.loads(payload)
                msg.body = body_dict
            except ValueError:
                msg.body = payload
        elif isinstance(payload, dict):
            msg.body = payload
        else:
            raise TypeError("Message payload must be a json string or dictionary")
        return msg

    @staticmethod
    def build(msg_contents, is_json: bool = True):
        ret_msg = Message()
        if is_json:
            if 'header' in msg_contents:
                ret_msg.header = MessageHeader.from_dict(msg_contents['header'])
            if 'source' in msg_contents:
                ret_msg.source = MessageSource.from_dict(msg_contents['source'])
            if 'body' in msg_contents:
                ret_msg.body = msg_contents['body']
            if not ('header' in msg_contents and 'source' in msg_contents and 'body' in msg_contents):
                ret_msg.raw = msg_contents
        else:
            ret_msg.raw = msg_contents
        return ret_msg

    @staticmethod
    def build_response(name: str, version: str, payload, config_manager: ConfigManager, request_msg: Message):
        return MessageBuilder.build_from_config(name, version, payload, config_manager,
                                                correlation_id=request_msg.get_correlation_id())
