"""
Builder for creating Message instances with fluent API.

``build()`` is the single UNS identity stamping site (UNS-CANONICAL-DESIGN §1.4): an
explicit :meth:`MessageBuilder.with_identity` override wins; otherwise, when a config
service is present, the component's resolved identity
(``ConfigManager.get_component_identity()``) is stamped with the per-message instance
token (:meth:`MessageBuilder.with_instance`, default ``"main"``); with neither,
``identity`` stays ``None`` (bootstrap/raw messages legally omit it).
"""
import json
from typing import TYPE_CHECKING, Optional

from ggcommons.messaging.identity import MessageIdentity

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
        self.instance = None
        self.identity_override: Optional[MessageIdentity] = None

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
            if "identity" in msg_contents:
                # Lenient wire parse: a malformed identity yields None (no override).
                builder.with_identity(MessageIdentity.from_dict(msg_contents["identity"]))
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
        """Pins the header ``uuid`` instead of the generated random one — deterministic
        envelopes for tests and the cross-language ``uns-test-vectors`` golden
        envelopes (D-U13)."""
        self.uuid = uuid
        return self

    def with_timestamp(self, timestamp: str) -> 'MessageBuilder':
        """Pins the header ``timestamp`` instead of the generated "now" — deterministic
        envelopes for tests and the cross-language ``uns-test-vectors`` golden
        envelopes (D-U13)."""
        self.timestamp = timestamp
        return self

    def with_reply_to(self, reply_to: str) -> 'MessageBuilder':
        self.reply_to = reply_to
        return self

    def with_instance(self, instance: str) -> 'MessageBuilder':
        """Sets the per-message instance token stamped into the identity element
        (default ``"main"``). Only takes effect when an identity is stamped (a config
        service is present; an explicit identity override is stamped verbatim).

        :raises ValueError: if ``instance`` is ``None`` or empty
        """
        if not instance:
            raise ValueError("instance must be non-empty")
        self.instance = instance
        return self

    def with_identity(self, identity: Optional[MessageIdentity]) -> 'MessageBuilder':
        """Sets an explicit identity override (tests, conformance vectors, relays).
        Wins over the config-resolved identity and is stamped verbatim (the
        :meth:`with_instance` token is not applied to an override)."""
        self.identity_override = identity
        return self

    def build(self):
        from ggcommons.messaging.message import Message, MessageHeader, MessageTags

        message = Message()
        message.header = MessageHeader(self.name, self.version, self.correlation_id,
                                       self.timestamp, self.uuid, self.reply_to)

        if self.tags is not None:
            message.tags = MessageTags.from_dict(self.tags)
        elif self.config_service is not None:
            message.tags = MessageTags.from_config(self.config_service)

        # The single identity stamping site (§1.4): explicit override > config-resolved
        # component identity (+ per-message instance token) > none (bootstrap/raw cases
        # stay valid).
        if self.identity_override is not None:
            message.identity = self.identity_override
        elif self.config_service is not None:
            component_identity = self.config_service.get_component_identity()
            if component_identity is not None:
                message.identity = component_identity.with_instance(
                    self.instance if self.instance else MessageIdentity.DEFAULT_INSTANCE
                )

        if isinstance(self.payload, str):
            try:
                message.body = json.loads(self.payload)
            except json.JSONDecodeError:
                message.body = self.payload
        else:
            message.body = self.payload

        return message
