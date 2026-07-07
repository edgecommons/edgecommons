"""The library-owned component **command inbox** — the minimal ``commands()`` facade
(DESIGN-uns §7.3/§9.5, the edge-console slice S2): every component subscribes, on its
PRIMARY (local/IPC) connection, its own ``main``-instance command-inbox wildcard::

    ecv1/{device}/{component}/main/cmd/#

and dispatches incoming ``cmd`` envelopes to handlers by **verb** — the topic's
channel (everything after ``cmd/``, ``/``-namespaced verbs included), which the
envelope's ``header.name`` must equal. A request carrying ``header.reply_to`` gets a
structured reply on that topic with the request's ``correlation_id`` (the
``uns-bridge`` rewrites ``reply_to`` across brokers, so console-to-component
request/reply works transparently over the site bus); a ``cmd`` without ``reply_to``
is fire-and-forget (the handler runs, no reply). Obtain the facade via
``EdgeCommons.get_commands()`` and register custom verbs with
:meth:`CommandInbox.register`.

Mirrors ``libs/java/.../commands/CommandInbox.java`` (the Java canonical); the wire
contract is pinned by ``uns-test-vectors/commands.json``.

**Normative behavior (identical across all four languages):**

- **Reply body shape** — success ``{"ok": true, "result": <verb-specific object>}``
  (a ``None`` handler result becomes the empty object ``{}`` — a plain ack); error
  ``{"ok": false, "error": {"code": <CODE>, "message": <text>}}``. The reply
  envelope's ``header.name`` is the verb, ``header.version`` is
  :data:`CMD_MESSAGE_VERSION`, and it carries the **responder's** ``identity`` (and
  ``tags``, when configured — metadata, not normative).
- **Built-in verbs** (registered by the library at construction; cannot be shadowed
  or unregistered): :data:`PING` -> ``{"status": "RUNNING", "uptimeSecs": n}``
  (liveness/echo, the state keepalive's RUNNING body shape); :data:`RELOAD_CONFIG` ->
  re-fetch/re-apply the configuration from the active config source
  (``{"reloaded": true}`` or :data:`ERR_RELOAD_FAILED`); :data:`GET_CONFIGURATION` ->
  return the current **redacted effective config** as
  ``{"config": <redacted config>}`` — the same redacted snapshot the ``cfg`` push
  class publishes, as a reply (**Flow B**: the console pulls a component's own
  config; unrelated to the Flow-A
  ``ecv1/{device}/config/main/cmd/get-configuration`` rendezvous where a component
  fetches its config FROM a config server).
- **Unknown verb** — a well-formed request whose verb has no handler gets an
  :data:`ERR_UNKNOWN_VERB` error reply (fire-and-forget unknowns are ignored at
  DEBUG).
- **Malformed** — a missing header, a ``header.name`` that does not equal the
  topic's verb, or any parse anomaly is ignored at DEBUG, **never replied to and
  never a crash** (the G-S1 precedent; replying would race foreign conventions that
  use a different header name on a ``cmd`` topic).
- **Delegated verbs** — :data:`SET_CONFIG_VERB` is owned by the
  ``CONFIG_COMPONENT`` config source's own subscription on the same inbox path; the
  inbox always ignores it (DEBUG) so the two subscribers never double-handle.
- **Handler errors** — a :class:`CommandException` keeps its code; any other
  exception maps to :data:`ERR_HANDLER_ERROR`. Fire-and-forget failures are logged
  only.
- **No config surface** — always on; core plumbing, not a feature toggle.

Lifecycle: constructed and :meth:`CommandInbox.start` started by the ``EdgeCommons``
runtime after initialization completes (right after the §9.4 republish listener);
:meth:`CommandInbox.close` unsubscribes the inbox (before messaging closes — the
unsubscribe-before-exit rule). When the component identity is not resolved (mock/test
bring-up), the inbox disables itself with a WARN, mirroring the heartbeat, the
effective-config publisher and the republish listener. Only the ``main``-instance
inbox is subscribed in this slice; per-instance inboxes ride the full ``commands()``
facade (Phase 5).
"""
import logging
import threading
from typing import TYPE_CHECKING, Callable, Dict, Optional, Set

from edgecommons.uns import Uns, UnsClass, UnsScope

if TYPE_CHECKING:
    from edgecommons.messaging.message import Message

logger = logging.getLogger("CommandInbox")

#: The liveness/echo built-in verb.
PING = "ping"

#: The re-fetch/re-apply-configuration built-in verb.
RELOAD_CONFIG = "reload-config"

#: The return-my-redacted-effective-config built-in verb (Flow B).
GET_CONFIGURATION = "get-configuration"

#: The command request/reply envelope version.
CMD_MESSAGE_VERSION = "1.0"

#: Error code: the request's verb has no registered handler on this component.
ERR_UNKNOWN_VERB = "UNKNOWN_VERB"

#: Error code: the handler threw an uncoded exception.
ERR_HANDLER_ERROR = "HANDLER_ERROR"

#: Error code: RELOAD_CONFIG could not re-fetch or the document was rejected.
ERR_RELOAD_FAILED = "RELOAD_FAILED"

#: Error code: GET_CONFIGURATION found no effective configuration to return.
ERR_NO_CONFIG = "NO_CONFIG"

#: The ``set-config`` push verb - delegated: the ``CONFIG_COMPONENT`` config source
#: maintains its own subscription for it on the same inbox path
#: (``ConfigComponentManager``), so the inbox must never dispatch or error-reply it.
SET_CONFIG_VERB = "set-config"

#: The built-in verbs (registered at construction; shadowing/unregistering is rejected).
BUILT_IN_VERBS = frozenset({PING, RELOAD_CONFIG, GET_CONFIGURATION})

#: Verbs owned by other library subscriptions on the same inbox path - always ignored.
DELEGATED_VERBS = frozenset({SET_CONFIG_VERB})

#: A command-verb handler: ``(request: Message) -> Optional[dict]``. The return value
#: is the verb-specific result object, wrapped by the inbox into the success reply
#: body; ``None`` yields an empty result (a plain acknowledgement). Raise
#: :class:`CommandException` for a coded error reply; any other exception becomes
#: :data:`ERR_HANDLER_ERROR`. Handlers run synchronously on the messaging delivery
#: thread - keep them fast, or hand off internally.
CommandHandler = Callable[["Message"], Optional[dict]]


class CommandException(Exception):
    """A coded command failure (DESIGN-uns §9.5): raised by a :data:`CommandHandler`
    to produce a structured error reply
    ``{"ok": false, "error": {"code": <code>, "message": <message>}}`` with a
    caller-chosen machine-readable code. Any *other* exception a handler raises is
    mapped to the generic :data:`ERR_HANDLER_ERROR` code - this class exists so a
    handler (built-in or custom) can distinguish its failure modes for the console
    (e.g. :data:`ERR_RELOAD_FAILED`, :data:`ERR_NO_CONFIG`)."""

    def __init__(self, code: str, message: Optional[str] = None):
        """
        :param code: the machine-readable error code (non-empty; SCREAMING_SNAKE_CASE
            by convention - see the pinned base codes on this module) carried in the
            error reply's ``error.code``
        :param message: the human-readable message carried in the error reply's
            ``error.message``
        """
        if not code:
            raise ValueError("code must not be empty")
        super().__init__(message)
        self.code = code
        self.message = message


class CommandInbox:
    """See the module docstring for the full normative behavior."""

    # Re-exported as class attributes for parity with the Java constants
    # (``CommandInbox.PING`` etc.) and for conformance tests.
    PING = PING
    RELOAD_CONFIG = RELOAD_CONFIG
    GET_CONFIGURATION = GET_CONFIGURATION
    CMD_MESSAGE_VERSION = CMD_MESSAGE_VERSION
    ERR_UNKNOWN_VERB = ERR_UNKNOWN_VERB
    ERR_HANDLER_ERROR = ERR_HANDLER_ERROR
    ERR_RELOAD_FAILED = ERR_RELOAD_FAILED
    ERR_NO_CONFIG = ERR_NO_CONFIG
    SET_CONFIG_VERB = SET_CONFIG_VERB
    BUILT_IN_VERBS = BUILT_IN_VERBS
    DELEGATED_VERBS = DELEGATED_VERBS

    def __init__(
        self,
        config_manager,
        messaging_client,
        uptime_secs: Callable[[], int],
        config_reload: Callable[[], bool],
        redacted_config: Callable[[], Optional[dict]],
    ):
        """Creates the inbox and registers the three built-in verbs. The verb
        *actions* are injected seams so the built-ins unit-test deterministically;
        the ``EdgeCommons`` runtime wires the real ones.

        :param config_manager: the component's config manager (own identity
            resolution; reply envelopes are config-stamped with the responder's
            identity/tags)
        :param messaging_client: the messaging handle (the ``MessagingClient`` class)
            whose PRIMARY connection carries the inbox
        :param uptime_secs: the :data:`PING` uptime source (production: the
            heartbeat's monotonic uptime, ``EnhancedHeartbeat.get_uptime_secs``)
        :param config_reload: the :data:`RELOAD_CONFIG` action - re-fetch + re-apply
            from the active config source, ``True`` on success (production:
            ``ConfigManager.reload_from_provider``)
        :param redacted_config: the :data:`GET_CONFIGURATION` source - the current
            redacted effective config, or ``None`` when unavailable (production:
            ``EffectiveConfigPublisher.redacted_effective_config``)
        """
        if config_manager is None:
            raise ValueError("config_manager must not be None")
        if messaging_client is None:
            raise ValueError("messaging_client must not be None")
        if uptime_secs is None:
            raise ValueError("uptime_secs must not be None")
        if config_reload is None:
            raise ValueError("config_reload must not be None")
        if redacted_config is None:
            raise ValueError("redacted_config must not be None")

        self._config_manager = config_manager
        self._messaging_client = messaging_client

        # verb -> handler; built-ins seeded here, custom verbs via register().
        self._handlers: Dict[str, CommandHandler] = {}

        # ping -> the state keepalive's RUNNING body shape: proves the component is
        # not just alive (the keepalive does that) but RESPONSIVE to addressed
        # commands.
        def _ping(request):
            return {"status": "RUNNING", "uptimeSecs": uptime_secs()}

        # reload-config -> re-fetch from the active config source and re-apply
        # (listeners fire, so a successful reload also re-announces the cfg push as
        # a side effect).
        def _reload_config(request):
            if not config_reload():
                raise CommandException(
                    ERR_RELOAD_FAILED,
                    "the configuration could not be re-fetched from the active"
                    " config source or the document was rejected - see the"
                    " component log",
                )
            return {"reloaded": True}

        # get-configuration (Flow B) -> the cfg class's body shape, as a reply.
        def _get_configuration(request):
            config = redacted_config()
            if config is None:
                raise CommandException(
                    ERR_NO_CONFIG, "no effective configuration is available"
                )
            return {"config": config}

        self._handlers[PING] = _ping
        self._handlers[RELOAD_CONFIG] = _reload_config
        self._handlers[GET_CONFIGURATION] = _get_configuration

        # The subscribed inbox filter (".../cmd/#"); None until start() builds it.
        self._inbox_filter: Optional[str] = None
        # The filter minus the trailing '#' - the verb-extraction prefix
        # (".../cmd/"); assigned BEFORE subscribing so a delivery racing the
        # subscribe call sees it.
        self._inbox_prefix: Optional[str] = None

        self._lock = threading.RLock()
        self._started = False
        self._closed = False

    def register(self, verb: str, handler: CommandHandler) -> None:
        """Registers a custom verb handler - the minimal ``commands()`` registration
        seam. The verb is one or more ``/``-separated channel tokens
        (``"restart-pipeline"``, ``"sb/status"``), each validated against the §2.2
        token rule. Registration is allowed before or after :meth:`start` (the inbox
        is a single wildcard subscription - no per-verb subscribe).

        **Precedence:** no shadowing, ever - registering a :data:`BUILT_IN_VERBS`
        built-in, a :data:`DELEGATED_VERBS` delegated verb, or an already-registered
        verb raises. Replace a custom handler by :meth:`unregister` first.

        :param verb: the verb (the ``cmd`` channel, ``/``-namespaces allowed)
        :param handler: the handler to dispatch it to
        :raises ValueError: when the verb is built-in/delegated/already registered
        :raises edgecommons.uns.UnsValidationError: when a verb token violates the
            §2.2 token rule
        """
        if verb is None:
            raise ValueError("verb must not be None")
        if handler is None:
            raise ValueError("handler must not be None")
        for token in verb.split("/"):
            Uns.check_token(token, "verb token")
        if verb in BUILT_IN_VERBS:
            raise ValueError(
                f"verb '{verb}' is a built-in verb and cannot be shadowed"
            )
        if verb in DELEGATED_VERBS:
            raise ValueError(
                f"verb '{verb}' is owned by another library subsystem and cannot"
                " be registered"
            )
        if verb in self._handlers:
            raise ValueError(
                f"verb '{verb}' is already registered - unregister it first to"
                " replace the handler"
            )
        self._handlers[verb] = handler
        logger.debug("Command verb '%s' registered", verb)

    def unregister(self, verb: str) -> None:
        """Removes a previously registered custom verb handler. Unknown verbs are a
        no-op; built-in verbs cannot be unregistered.

        :raises ValueError: when the verb is a built-in
        """
        if verb is None:
            raise ValueError("verb must not be None")
        if verb in BUILT_IN_VERBS:
            raise ValueError(
                f"verb '{verb}' is a built-in verb and cannot be unregistered"
            )
        if self._handlers.pop(verb, None) is not None:
            logger.debug("Command verb '%s' unregistered", verb)

    def verbs(self) -> Set[str]:
        """The currently registered verbs (built-ins + custom) - a snapshot copy."""
        return set(self._handlers.keys())

    def start(self) -> None:
        """Builds the own-inbox wildcard (``ecv1/{device}/{component}/main/cmd/#``,
        through the topic builder under this component's identity + root mode) and
        subscribes it on the PRIMARY connection. Best-effort and idempotent: with no
        resolved component identity (mock/test bring-up) - or on any subscription
        failure - the inbox logs and disables itself; the component must come up
        regardless."""
        with self._lock:
            if self._started or self._closed:
                return
            identity = self._config_manager.get_component_identity()
            if identity is None:
                logger.warning(
                    "No resolved component identity - the command inbox is disabled"
                )
                return
            try:
                uns = Uns(identity, self._config_manager.is_topic_include_root())
                # Pin every scope token to this component's own identity: the site
                # value is consulted only under an effective root mode (D-U25 makes
                # it a no-op otherwise).
                site = identity.hier[0].value if len(identity.hier) >= 2 else None
                scope = UnsScope(site, identity.device, identity.component, identity.instance)
                filter_ = uns.filter(UnsClass.CMD, scope)
                self._inbox_filter = filter_
                # ".../cmd/#" -> ".../cmd/" - the verb is the topic's remainder
                # after this prefix.
                self._inbox_prefix = filter_[:-1]
                self._messaging_client.subscribe(filter_, self._handle)
                self._started = True
                logger.info(
                    "Command inbox subscribed on '%s' (verbs: %s)",
                    filter_,
                    sorted(self._handlers.keys()),
                )
            except Exception as e:  # noqa: BLE001 - best-effort by design
                logger.warning(
                    "Failed to start the command inbox (continuing without it): %s", e
                )

    def _handle(self, topic: Optional[str], message) -> None:
        """One received ``cmd`` envelope: extract the verb from the topic, validate
        the envelope (``header.name`` must equal the verb), dispatch, reply. Never
        raises - a malformed or foreign payload is ignored at DEBUG."""
        try:
            with self._lock:
                if self._closed:
                    return
            if topic is None or not topic.startswith(self._inbox_prefix):
                # ".../cmd/#" also matches the bare ".../cmd" parent level - nothing
                # to dispatch.
                logger.debug("Ignoring cmd delivery outside the inbox prefix: '%s'", topic)
                return
            verb = topic[len(self._inbox_prefix):]
            if not verb:
                return
            if verb in DELEGATED_VERBS:
                logger.debug(
                    "Ignoring delegated verb '%s' (owned by another library"
                    " subscription)",
                    verb,
                )
                return
            header = message.get_header() if message is not None else None
            if header is None or verb != header.name:
                # Malformed/foreign: never replied to (a reply would race foreign
                # conventions using a different header name on a cmd topic), never
                # a crash.
                logger.debug(
                    "Ignoring malformed/foreign cmd payload on '%s' (header.name"
                    " must equal the topic verb)",
                    topic,
                )
                return
            self._dispatch(verb, message)
        except Exception as e:  # noqa: BLE001 - a bad payload must never crash us
            logger.debug("Ignoring malformed cmd payload on '%s': %s", topic, e)

    def _dispatch(self, verb: str, request) -> None:
        """Dispatches a well-formed request to its handler and replies (when
        ``reply_to`` is set)."""
        wants_reply = bool(request.get_header().reply_to)
        handler = self._handlers.get(verb)
        if handler is None:
            if wants_reply:
                logger.debug(
                    "Unknown verb '%s' - sending %s error reply", verb, ERR_UNKNOWN_VERB
                )
                self._send_reply(
                    request,
                    verb,
                    _error_body(
                        ERR_UNKNOWN_VERB,
                        f"verb '{verb}' is not registered on this component",
                    ),
                )
            else:
                logger.debug("Ignoring unknown fire-and-forget verb '%s'", verb)
            return
        try:
            result = handler(request)
        except CommandException as e:
            if wants_reply:
                self._send_reply(request, verb, _error_body(e.code, e.message))
            else:
                logger.warning(
                    "Fire-and-forget verb '%s' failed (%s): %s", verb, e.code, e.message
                )
            return
        except Exception as e:  # noqa: BLE001 - any other handler failure
            if wants_reply:
                self._send_reply(request, verb, _error_body(ERR_HANDLER_ERROR, str(e)))
            else:
                logger.warning("Fire-and-forget verb '%s' failed: %s", verb, e)
            return
        if wants_reply:
            body = {"ok": True, "result": result if result is not None else {}}
            self._send_reply(request, verb, body)

    def _send_reply(self, request, verb: str, body: dict) -> None:
        """Publishes a reply to the request's ``reply_to`` through the existing
        reply mechanism (the provider stamps the request's ``correlation_id`` onto
        the reply). The reply is config-stamped, so it carries the responder's
        ``identity`` (+ ``tags``). Best-effort: a failing reply (e.g. a hostile
        reserved-class ``reply_to`` rejected by the guard) is logged and
        swallowed."""
        try:
            from edgecommons.messaging.message_builder import MessageBuilder

            reply = (
                MessageBuilder.create(verb, CMD_MESSAGE_VERSION)
                .with_command(body)
                .with_config(self._config_manager)
                .build()
            )
            self._messaging_client.reply(request, reply)
        except Exception as e:  # noqa: BLE001 - best-effort by design
            logger.warning("Command reply for verb '%s' failed: %s", verb, e)

    def close(self) -> None:
        """Stops the inbox: unsubscribes the inbox wildcard (while messaging is
        still up - the unsubscribe-before-exit rule) and stops dispatching.
        Idempotent."""
        with self._lock:
            if self._closed:
                return
            self._closed = True
            started = self._started
            inbox_filter = self._inbox_filter
        if started and inbox_filter is not None:
            try:
                self._messaging_client.unsubscribe(inbox_filter)
            except Exception as e:  # noqa: BLE001 - best-effort by design
                logger.debug(
                    "Command-inbox unsubscribe of '%s' failed: %s", inbox_filter, e
                )


def _error_body(code: str, message: Optional[str]) -> dict:
    """The error reply body ``{"ok": false, "error": {"code", "message"}}``."""
    return {"ok": False, "error": {"code": code, "message": message if message is not None else ""}}
