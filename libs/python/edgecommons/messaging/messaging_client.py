import logging
from argparse import Namespace
from typing import Callable, Optional, Union

from edgecommons.messaging.errors import ReservedTopicError
from edgecommons.messaging.message import Message
from edgecommons.messaging.messaging_provider import MessagingProvider
from edgecommons.messaging.messaging_config import MessagingConfiguration
from edgecommons.messaging.qos import Qos
from edgecommons.messaging.providers.greengrass.greengrass_ipc import (
    GreengrassIpcProvider,
)
from edgecommons.messaging.providers.standalone_provider import StandaloneProvider
from edgecommons.platform import Transport
from edgecommons.uns import Uns, RESERVED_CLASSES
from edgecommons.utils.iou import Iou

logger = logging.getLogger("MessagingClient")

# The reserved UNS class tokens (state | metric | cfg | log) as plain strings, for the
# §4.1 guard predicate's token comparisons.
_RESERVED_TOKENS = frozenset(cls.value for cls in RESERVED_CLASSES)


class MessagingClient:
    _messaging_provider: MessagingProvider = None

    # Whether the reserved-class publish guard also checks the class token at topic
    # position 5 — this component's EFFECTIVE root mode (topic.includeRoot AND a
    # multi-level hierarchy — UNS-CANONICAL-DESIGN §4.1, D-U24/D-U27). Late-bound from
    # the ConfigManager via set_guard_include_root() right after config loads (the
    # messaging client is initialized BEFORE config because the IPC-backed config
    # sources need it); False pre-bind — nothing publishes rooted topics pre-config.
    _guard_include_root: bool = False

    @staticmethod
    def init(args: Namespace, standalone_config_path: str = None, receive_own_messages=False) -> MessagingProvider:
        """Initialize the messaging client based on the resolved transport.

        Branches on the resolved :class:`~edgecommons.platform.transport.Transport`
        (DESIGN-core sec 4.2 transport-injection site), not on a legacy mode token.
        """
        transport = getattr(args, 'transport', None)
        # Use the identity resolved by the platform resolver (canonical precedence
        # -t > AWS_IOT_THING_NAME > NOT_GREENGRASS), not the raw -t flag. Fall back to
        # the raw flag only for callers that bypass the resolver.
        thing_name = getattr(args, 'identity', None) or getattr(args, 'thing', None)

        logger.info(f"Initializing MessagingClient - transport: {transport}, thing_name: {thing_name}, receive_own_messages: {receive_own_messages}")

        if transport == Transport.MQTT:
            logger.info(f"MQTT transport selected - dual broker support, config file: {standalone_config_path}")
            # The messaging-config path is required only when the MQTT provider is actually built,
            # mirroring how the IPC provider only fails against a live Nucleus.
            if not standalone_config_path:
                logger.error("MQTT transport specified but no messaging config file path provided")
                raise RuntimeError("MQTT transport requires a messaging config file path")

            logger.debug(f"Loading messaging configuration from: {standalone_config_path}")
            messaging_config = MessagingClient._get_messaging_config(standalone_config_path)

            logger.info("Creating StandaloneProvider for dual broker messaging")
            MessagingClient._messaging_provider = StandaloneProvider(messaging_config, thing_name)
            logger.info("MQTT transport messaging provider initialized successfully")
        elif transport == Transport.IPC:
            logger.info(f"IPC transport selected - Greengrass IPC, receive_own_messages: {receive_own_messages}")
            MessagingClient._messaging_provider = GreengrassIpcProvider(
                receive_own_messages
            )
            logger.info("Greengrass IPC messaging provider initialized successfully")
        else:
            logger.error(f"Invalid transport specified: {transport}")
            raise RuntimeError(f"Invalid transport specified: {transport}")

        if MessagingClient._messaging_provider is None:
            logger.error("Failed to create messaging provider - provider is None")
            raise RuntimeError("Failed to initialize messaging provider")
        
        logger.info(f"MessagingClient initialization completed - provider type: {type(MessagingClient._messaging_provider).__name__}")
        return MessagingClient._messaging_provider
    
    @staticmethod
    def _get_messaging_config(standalone_config_path: str) -> MessagingConfiguration:
        """Get messaging configuration from standalone config file."""
        logger.debug(f"Loading messaging configuration from file: {standalone_config_path}")
        
        try:
            config = MessagingConfiguration.load_from_file(standalone_config_path)
            logger.debug(f"Successfully loaded messaging configuration from {standalone_config_path}")
            
            logger.debug("Validating messaging configuration")
            if not config.validate():
                logger.error(f"Messaging configuration validation failed for file: {standalone_config_path}")
                raise RuntimeError("Invalid messaging configuration")
            
            logger.info(f"Messaging configuration loaded and validated successfully from: {standalone_config_path}")
            return config
            
        except Exception as e:
            logger.error(f"Failed to load messaging configuration from {standalone_config_path}: {e}")
            raise RuntimeError(f"STANDALONE mode requires valid messaging configuration: {e}")

    @staticmethod
    def shutdown():
        # Idempotent: safe to call more than once (e.g. EdgeCommons.shutdown plus a
        # caller's own cleanup).
        if MessagingClient._messaging_provider is not None:
            MessagingClient._messaging_provider.disconnect()
            MessagingClient._messaging_provider = None
        # Reset the guard's late-bound flag with the rest of the process-global state
        # (test hygiene for the static client; Java's per-instance client needs none).
        MessagingClient._guard_include_root = False

    @staticmethod
    def get_messaging_provider() -> MessagingProvider:
        return MessagingClient._messaging_provider

    @staticmethod
    def connected() -> bool:
        """Whether messaging currently has a usable connection (backs ``/readyz``, FR-HB-1).

        Returns ``False`` when no provider is wired (treated as not connected -> not ready) or when the
        provider reports/raises a non-connected state. Never raises.
        """
        provider = MessagingClient._messaging_provider
        if provider is None:
            return False
        try:
            return bool(provider.connected())
        except Exception:  # noqa: BLE001 - readiness check must never raise
            return False

    # ----- reserved-class publish guard (UNS-CANONICAL-DESIGN §4.1) --------------------

    @staticmethod
    def set_guard_include_root(include_root: bool) -> None:
        """Late-binds the reserved-class guard's root flag from the config model (§4.1,
        D-U24). The runtime binds the EFFECTIVE root — ``topic.includeRoot`` AND a
        multi-level hierarchy (D-U27), the same rule topic-building uses (D-U25) —
        right after the ConfigManager exists; before the bind only the always-checked
        class position 4 applies."""
        MessagingClient._guard_include_root = bool(include_root)
        logger.debug(f"Reserved-topic guard includeRoot bound to {include_root}")

    @staticmethod
    def _reserved_class_of(topic: Optional[str], include_root: bool) -> Optional[str]:
        """The §4.1 guard predicate: the reserved class token the topic targets, or
        ``None`` when the topic is allowed. The class position is topic level 4
        (0-based) always — the rootless grammar
        ``ecv1/{device}/{component}/{instance}/{class}`` — and level 5 **only when
        this component's effective root mode is true** (checking it unconditionally
        would false-positive on legitimate app channels like ``ecv1/d/c/i/app/state``).
        Non-``ecv1`` topics pass untouched (``edgecommons/reply-...``,
        ``cloudwatch/metric/put``, foreign MQTT bridging)."""
        if not topic or not topic.startswith(Uns.ROOT):
            return None
        tokens = topic.split("/")
        if tokens[0] != Uns.ROOT:
            return None
        if len(tokens) >= 5 and tokens[4] in _RESERVED_TOKENS:
            return tokens[4]
        if include_root and len(tokens) >= 6 and tokens[5] in _RESERVED_TOKENS:
            return tokens[5]
        return None

    @staticmethod
    def _check_reserved_topic(topic: Optional[str]) -> None:
        """Rejects a client-chosen topic whose class position holds a reserved token
        (``state | metric | cfg | log``). ``subscribe*`` is never guarded (consumers
        must read reserved classes).

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        reserved = MessagingClient._reserved_class_of(
            topic, MessagingClient._guard_include_root
        )
        if reserved is not None:
            raise ReservedTopicError(topic, reserved)

    @staticmethod
    def _reply_topic_of(request: Optional[Message]) -> Optional[str]:
        """The request's ``reply_to`` topic, or ``None`` when it has no header/reply-to."""
        if request is None or request.get_header() is None:
            return None
        return request.get_header().reply_to

    # ----- privileged internal-publish seam (UNS-CANONICAL-DESIGN §4.2, D-U4) ----------

    @staticmethod
    def _publish_reserved(topic: str, msg: Message):
        """Unguarded local/IPC publish — the privileged internal-publish seam (§4.2).
        **Library-internal** (underscore convention): only the library's own publishers
        (heartbeat/state keepalive, the ``messaging`` metric target, the effective-
        config publisher) may use it. The guard it bypasses is misuse prevention, not a
        security boundary (broker ACLs are)."""
        MessagingClient._messaging_provider.publish(topic, msg)
        logger.debug(f"Published reserved message on topic '{topic}'")

    @staticmethod
    def _publish_reserved_raw(topic: str, msg: dict):
        """Unguarded raw local/IPC publish — the privileged seam (§4.2)."""
        MessagingClient._messaging_provider.publish_raw(topic, msg)

    @staticmethod
    def _publish_reserved_northbound(topic: str, msg: Message, qos):
        """Unguarded northbound publish — the privileged seam (§4.2)."""
        MessagingClient._messaging_provider.publish_northbound(topic, msg, qos)
        logger.debug(f"Published reserved northbound message on topic '{topic}'")

    # ----- public messaging surface (guarded) -------------------------------------------

    @staticmethod
    def publish(topic: str, msg: Message):
        """Publishes a message. Client-chosen topics targeting a reserved UNS class
        (``state | metric | cfg | log``) are rejected (§4.1) — the library publishers
        own those classes.

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        MessagingClient._check_reserved_topic(topic)
        logger.debug(f"Publishing message to topic: {topic}")
        MessagingClient._messaging_provider.publish(topic, msg)

    @staticmethod
    def publish_confirmed(
        topic: str,
        message: Union[Message, bytes],
        qos: Qos,
        timeout_secs: float,
    ) -> None:
        """Strict local publication with positive QoS-1 transport confirmation.

        ``message`` may be a :class:`Message` or exact serialized envelope bytes.
        Durable outboxes use the byte form so retries preserve every byte and the
        envelope UUID.  Timeout, disconnect, and unsupported transports raise.
        """
        MessagingClient._check_reserved_topic(topic)
        if isinstance(message, Message):
            encoded = message.to_bytes()
        elif isinstance(message, bytes):
            encoded = message
        else:
            raise TypeError("message must be a Message or exact bytes")
        MessagingClient._validate_confirmed_envelope(encoded)
        MessagingClient._messaging_provider.publish_confirmed(
            topic, encoded, qos, timeout_secs
        )

    @staticmethod
    def _validate_confirmed_envelope(encoded: bytes) -> None:
        """Parses exact outbox bytes through the canonical envelope codec.

        Validation never replaces the caller's representation: providers still
        receive the original bytes so retries remain byte-for-byte identical.
        """
        try:
            Message.from_bytes(encoded).to_bytes()
        except Exception as exc:  # noqa: BLE001 - normalize codec failures for callers
            raise ValueError(
                "confirmed publish requires a valid EdgeCommons envelope"
            ) from exc

    @staticmethod
    def publish_raw(topic: str, msg: dict):
        """Publishes a raw dict. Reserved-class UNS topics are rejected (§4.1, D-U8).

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        MessagingClient._check_reserved_topic(topic)
        MessagingClient._messaging_provider.publish_raw(topic, msg)

    @staticmethod
    def publish_northbound(topic: str, msg: Message, qos: Qos):
        """Publishes to the northbound transport. Reserved-class UNS topics are rejected (§4.1).

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        MessagingClient._check_reserved_topic(topic)
        logger.debug(f"Publishing message to northbound topic: {topic}, QoS: {qos}")
        MessagingClient._messaging_provider.publish_northbound(topic, msg, qos)

    @staticmethod
    def publish_northbound_confirmed(
        topic: str,
        message: Union[Message, bytes],
        qos: Qos,
        timeout_secs: float,
    ) -> None:
        """Strict northbound publication of a message or exact envelope bytes."""
        MessagingClient._check_reserved_topic(topic)
        if isinstance(message, Message):
            encoded = message.to_bytes()
        elif isinstance(message, bytes):
            encoded = message
        else:
            raise TypeError("message must be a Message or exact bytes")
        MessagingClient._validate_confirmed_envelope(encoded)
        MessagingClient._messaging_provider.publish_northbound_confirmed(
            topic, encoded, qos, timeout_secs
        )

    @staticmethod
    def publish_northbound_raw(topic: str, msg: dict, qos: Qos):
        """Raw northbound publish. Reserved-class UNS topics are rejected (§4.1, D-U8).

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        MessagingClient._check_reserved_topic(topic)
        MessagingClient._messaging_provider.publish_northbound_raw(topic, msg, qos)

    @staticmethod
    def subscribe(
        topic: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        logger.debug(f"Subscribing to topic: {topic}, max_concurrency: {max_concurrency}, max_messages: {max_messages}")
        MessagingClient._messaging_provider.subscribe(topic, callback, max_concurrency, max_messages)

    @staticmethod
    def subscribe_acknowledged(
        topic: str,
        callback: Callable[[str, Message], None],
        max_concurrency: int = None,
        max_messages: int = None,
        timeout_secs: float = 10.0,
    ) -> None:
        """Lifecycle-critical local subscribe with positive transport acknowledgement."""

        provider = MessagingClient._messaging_provider
        if provider is None:
            raise RuntimeError("messaging provider is not initialized")
        provider.subscribe_acknowledged(
            topic,
            callback,
            max_concurrency,
            max_messages,
            timeout_secs,
        )

    @staticmethod
    def subscribe_northbound(
        topic: str,
        callback: Callable[[str, Message], None],
        qos: Qos,
        max_concurrency: int = None,
        max_messages: int = None,
    ):
        logger.debug(f"Subscribing to northbound topic: {topic}, QoS: {qos}, max_concurrency: {max_concurrency}, max_messages: {max_messages}")
        MessagingClient._messaging_provider.subscribe_northbound(
            topic, callback, qos, max_concurrency, max_messages
        )

    @staticmethod
    def unsubscribe(topic: str):
        MessagingClient._messaging_provider.unsubscribe(topic)

    @staticmethod
    def unsubscribe_northbound(topic: str):
        MessagingClient._messaging_provider.unsubscribe_northbound(topic)

    @staticmethod
    def request(topic: str, msg: Message, timeout_secs: Optional[float] = None) -> Iou:
        """Sends a request and returns the reply :class:`Iou`. The Iou carries the
        framework-owned default deadline (``messaging.requestTimeoutSeconds``, default
        30 s, UNS-CANONICAL-DESIGN §5): on expiry the ephemeral reply subscription is
        cleaned up and ``Iou.get()`` raises
        :class:`~edgecommons.messaging.errors.RequestTimeoutError` — even if the caller
        never ``get()``'s it. ``timeout_secs``: ``None`` = the configured default,
        ``0`` = deadline disabled for this call, an explicit value always wins.

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        MessagingClient._check_reserved_topic(topic)
        logger.debug(f"Sending request to topic: {topic}")
        return MessagingClient._messaging_provider.request(topic, msg, timeout_secs)

    @staticmethod
    def request_northbound(topic: str, msg: Message,
                              timeout_secs: Optional[float] = None) -> Iou:
        """Northbound variant of :meth:`request` (same deadline + guard semantics).

        :raises ReservedTopicError: when the topic targets a reserved UNS class
        """
        MessagingClient._check_reserved_topic(topic)
        logger.debug(f"Sending request to northbound topic: {topic}")
        return MessagingClient._messaging_provider.request_northbound(
            topic, msg, timeout_secs
        )

    @staticmethod
    def set_default_request_timeout(timeout_secs: Optional[float]) -> None:
        """Late-binds the default ``request()`` deadline from the config model
        (``messaging.requestTimeoutSeconds``, §5/D-U5). Called by the runtime right
        after the ConfigManager exists (the messaging client is initialized first
        because the IPC-backed config sources need it); until then the built-in 30 s
        applies — deliberately, so the CONFIG_COMPONENT bootstrap request gets a
        deadline instead of hanging. ``None``/``0`` disables the default deadline.
        Safe no-op when no provider is wired."""
        provider = MessagingClient._messaging_provider
        if provider is not None:
            provider.set_default_request_timeout(timeout_secs)
            logger.debug(f"Default request timeout bound to {timeout_secs}")

    @staticmethod
    def get_default_request_timeout() -> Optional[float]:
        """The default ``request()`` deadline currently in effect on the underlying
        provider in seconds (``0`` = disabled), or ``None`` when no provider is wired."""
        provider = MessagingClient._messaging_provider
        return None if provider is None else provider.get_default_request_timeout()

    @staticmethod
    def cancel_request(iou: Iou) -> Iou:
        return MessagingClient._messaging_provider.cancel_request(iou)

    @staticmethod
    def cancel_request_northbound(iou: Iou) -> Iou:
        return MessagingClient._messaging_provider.cancel_request_northbound(iou)

    @staticmethod
    def reply(request: Message, reply: Message):
        """Sends a reply to a received request. The request's ``reply_to`` topic is
        guarded like a client-chosen topic (§4.1, D-U8): a hostile requester could
        otherwise set ``header.reply_to`` to a victim's reserved topic and turn an
        innocent responder into a forger.

        :raises ReservedTopicError: when the request's reply topic targets a reserved
            UNS class
        """
        MessagingClient._check_reserved_topic(MessagingClient._reply_topic_of(request))
        MessagingClient._messaging_provider.reply(request, reply)

    @staticmethod
    def validate_reply_target(request: Message) -> str:
        """Validates and returns a received request's guarded ``reply_to`` topic.

        Deferred registries call this before retaining any request metadata so a
        missing or hostile target never becomes server-side reply state.
        """
        topic = MessagingClient._reply_topic_of(request)
        if not topic:
            raise ValueError("request requires a non-empty reply_to")
        MessagingClient._check_reserved_topic(topic)
        return topic

    @staticmethod
    def reply_confirmed(
        request: Message, reply: Message, timeout_secs: float
    ) -> None:
        """Sends a guarded local reply and waits for QoS-1 confirmation."""
        topic = MessagingClient.validate_reply_target(request)
        if reply is None:
            raise ValueError("reply must not be None")
        reply.set_correlation_id(request.get_correlation_id())
        MessagingClient.publish_confirmed(
            topic, reply, Qos.AT_LEAST_ONCE, timeout_secs
        )

    @staticmethod
    def reply_northbound(request: Message, reply: Message):
        """Northbound variant of :meth:`reply` — the request's ``reply_to`` topic is
        guarded the same way.

        :raises ReservedTopicError: when the request's reply topic targets a reserved
            UNS class
        """
        MessagingClient._check_reserved_topic(MessagingClient._reply_topic_of(request))
        MessagingClient._messaging_provider.reply_northbound(request, reply)

    @staticmethod
    def reply_northbound_confirmed(
        request: Message, reply: Message, timeout_secs: float
    ) -> None:
        """Guarded northbound counterpart of :meth:`reply_confirmed`."""
        topic = MessagingClient.validate_reply_target(request)
        if reply is None:
            raise ValueError("reply must not be None")
        reply.set_correlation_id(request.get_correlation_id())
        MessagingClient.publish_northbound_confirmed(
            topic, reply, Qos.AT_LEAST_ONCE, timeout_secs
        )

    @staticmethod
    def topic_matches_sub(sub: str, topic: str) -> bool:
        return MessagingProvider.topic_matches_sub(sub, topic)

    @staticmethod
    def get_native_client():
        return MessagingClient._messaging_provider.get_native_client()
