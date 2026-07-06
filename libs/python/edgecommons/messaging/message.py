# (c) 2022 Amazon Web Services, Inc. or its affiliates. All Rights Reserved.
# This AWS Content is provided subject to the terms of the AWS Customer Agreement
# available at http://aws.amazon.com/agreement or other written agreement between
# Customer and Amazon Web Services, Inc.

import base64
import binascii
import json
import logging
from dataclasses import dataclass, field
from uuid import uuid4
from typing import Any, Optional, TYPE_CHECKING

from edgecommons.messaging.identity import MessageIdentity
from edgecommons.utils import Utils


MAX_BINARY_BODY_BYTES = 64 * 1024
BINARY_BODY_KEY = "_edgecommonsBinary"
BINARY_ENCODING = "base64"


def _binary_marker(data: bytes) -> dict:
    if len(data) > MAX_BINARY_BODY_BYTES:
        raise ValueError(f"Binary message body exceeds {MAX_BINARY_BODY_BYTES} bytes")
    return {
        BINARY_BODY_KEY: {
            "encoding": BINARY_ENCODING,
            "length": len(data),
            "data": base64.b64encode(data).decode("ascii"),
        }
    }


def _encode_body(body: Any) -> Any:
    """Encode a top-level binary body as the first-class bounded binary marker."""
    if isinstance(body, (bytes, bytearray)):
        return _binary_marker(bytes(body))
    return body


def _binary_descriptor(value: Any) -> Optional[dict]:
    if isinstance(value, dict):
        if BINARY_BODY_KEY not in value:
            return None
        descriptor = value[BINARY_BODY_KEY]
        if not isinstance(descriptor, dict):
            raise ValueError("Binary message body marker must be an object")
        return descriptor
    return None


def _decode_binary_descriptor(descriptor: dict) -> bytes:
    if descriptor.get("encoding") != BINARY_ENCODING:
        raise ValueError("Binary message body encoding must be base64")
    declared_length = descriptor.get("length")
    if not isinstance(declared_length, int) or declared_length < 0:
        raise ValueError("Binary message body length must be a non-negative integer")
    if declared_length > MAX_BINARY_BODY_BYTES:
        raise ValueError(f"Binary message body exceeds {MAX_BINARY_BODY_BYTES} bytes")
    encoded = descriptor.get("data")
    if not isinstance(encoded, str):
        raise ValueError("Binary message body data is required")
    try:
        decoded = base64.b64decode(encoded, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise ValueError("Binary message body data is not valid base64") from exc
    if len(decoded) != declared_length:
        raise ValueError("Binary message body length does not match decoded data")
    return decoded


if TYPE_CHECKING:
    from edgecommons.config.manager.config_manager import ConfigManager


logger = logging.getLogger("Message")


@dataclass
class MessageHeader:
    REPLY_MESSAGE_TOPIC_PREFIX = "edgecommons/reply-"  # class constant, not a field

    name: str
    version: str
    correlation_id: Optional[str] = None
    timestamp: Optional[str] = None
    uuid: Optional[str] = None
    reply_to: Optional[str] = None

    def __post_init__(self):
        # Fill computed defaults (matches the previous hand-written constructor).
        if self.timestamp is None:
            self.timestamp = Utils.get_utc_z()
        if self.correlation_id is None:
            self.correlation_id = str(uuid4())
        if self.uuid is None:
            self.uuid = str(uuid4())

    @staticmethod
    def from_dict(src: dict):
        name = src.get("name")
        version = src.get("version")
        timestamp = src.get("timestamp")
        uuid = src.get("uuid")
        correlation_id = src.get("correlation_id")
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


@dataclass
class MessageTags:
    """Free-form message metadata tags.

    The legacy ``thing`` special-casing is removed (UNS-CANONICAL-DESIGN §1.1 — hard
    cut): the publisher's device now travels in the top-level ``identity`` element. A
    stray inbound ``thing`` key just lands in the generic tag map — no legacy shim.
    """

    tags: dict = field(default_factory=dict)

    def __post_init__(self):
        if self.tags is None:
            self.tags = {}

    @staticmethod
    def from_config(config_service: 'ConfigManager'):
        tag_config = config_service.get_tag_config()
        if tag_config is not None:
            return MessageTags(tag_config.to_dict())
        else:
            return MessageTags({})

    @staticmethod
    def from_dict(src: dict):
        return MessageTags(dict(src))

    def inject_tag(self, key: str, value: str):
        self.tags[key] = value

    def to_dict(self) -> dict:
        return dict(self.tags)


@dataclass
class Message:
    header: Optional[MessageHeader] = None
    identity: Optional[MessageIdentity] = None
    tags: Optional[MessageTags] = None
    body: Any = None
    raw: Any = None

    def to_dict(self) -> dict:
        if self.raw is None:
            msg = {}
            if self.header is not None:
                msg["header"] = self.header.to_dict()
            # Canonical envelope member order: header, identity, tags, body.
            if self.identity is not None:
                msg["identity"] = self.identity.to_dict()
            if self.tags is not None:
                msg["tags"] = self.tags.to_dict()
            msg["body"] = _encode_body(self.body)
            return msg
        else:
            return {"raw": _encode_body(self.raw)}

    def __str__(self) -> str:
        return json.dumps(self.to_dict())

    def dumps(self, indent=None) -> str:
        msg = {}
        if self.header is not None:
            msg["header"] = self.header.to_dict()
        if self.identity is not None:
            msg["identity"] = self.identity.to_dict()
        if self.tags is not None:
            msg["tags"] = self.tags.to_dict()
        msg["body"] = _encode_body(self.body)
        return json.dumps(msg, indent=indent)

    def get_correlation_id(self) -> str:
        if self.header is None:
            return None
        return self.header.correlation_id

    def get_header(self) -> MessageHeader:
        return self.header

    def get_identity(self) -> Optional[MessageIdentity]:
        """The UNS identity element of this message (``hier``/``path``/``component``/
        ``instance``), or ``None`` when the message carries none (raw messages,
        messages built without a config-bound builder, or a malformed inbound
        identity)."""
        return self.identity

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

    def is_binary_body(self) -> bool:
        return isinstance(self.body, (bytes, bytearray)) or (
            isinstance(self.body, dict) and BINARY_BODY_KEY in self.body
        )

    def get_binary_body(self) -> Optional[bytes]:
        if isinstance(self.body, (bytes, bytearray)):
            data = bytes(self.body)
            if len(data) > MAX_BINARY_BODY_BYTES:
                raise ValueError(f"Binary message body exceeds {MAX_BINARY_BODY_BYTES} bytes")
            return data
        descriptor = _binary_descriptor(self.body)
        return None if descriptor is None else _decode_binary_descriptor(descriptor)

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
                message.header = MessageHeader.from_dict(msg_contents["header"])
            if "identity" in msg_contents:
                # Lenient: a malformed identity yields None + a WARN and the message
                # still delivers (UNS-CANONICAL-DESIGN §1.2).
                message.identity = MessageIdentity.from_dict(msg_contents["identity"])
            if "tags" in msg_contents:
                message.tags = MessageTags.from_dict(msg_contents["tags"])
            if "body" in msg_contents:
                message.body = msg_contents["body"]
            if not any(key in msg_contents for key in ["header", "identity", "tags", "body"]):
                logger.debug("Dict contained raw data: Assigning to raw")
                message.raw = msg_contents
        else:
            logger.debug("Message not instance of dict, assigning to raw")
            message.raw = msg_contents

        return message
