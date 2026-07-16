"""The library-owned component **command inbox** — the minimal ``commands()`` facade
(DESIGN-uns §7.3/§9.5, the edge-console slice S2): every component subscribes, on its
PRIMARY (local/IPC) connection, BOTH its component-scope and instance-scope
command-inbox wildcards (D-U28)::

    ecv1/{device}/{component}/cmd/#      (component scope)
    ecv1/{device}/{component}/+/cmd/#    (any instance)

and dispatches incoming ``cmd`` envelopes to handlers by **verb** — the topic's
channel (everything after the ``/cmd/`` marker, ``/``-namespaced verbs included),
which the
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
  ``ecv1/{device}/config/cmd/get-configuration`` rendezvous where a component
  fetches its config FROM a config server); :data:`STATUS` -> ``ping``'s per-instance
  superset ``{"status": "RUNNING", "uptimeSecs": n[, "instances": [...]]}``, whose
  ``instances[]`` is the very sample the ``state`` keepalive pushes (one
  component-supplied provider, two surfaces - a pulled answer can never disagree with a
  pushed one); the section is omitted when the component reports no instances.
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
- **Deferred handlers** — :meth:`CommandInbox.register_outcome` adds explicit
  :class:`ImmediateSuccess`, :class:`ImmediateError`, and :class:`Deferred` outcomes
  without changing legacy handlers. The inbox owns the bounded, timed, guarded reply
  registry and an opaque :class:`DeferredReply` settles at most once.
- **No config surface** — always on; core plumbing, not a feature toggle.

Lifecycle: constructed and :meth:`CommandInbox.start` started by the ``EdgeCommons``
runtime after initialization completes (right after the §9.4 republish listener);
:meth:`CommandInbox.close` unsubscribes the inbox (before messaging closes — the
unsubscribe-before-exit rule). When the component identity is not resolved (mock/test
bring-up), the inbox disables itself with a WARN, mirroring the heartbeat, the
effective-config publisher and the republish listener. Both the component-scope
(``.../cmd/#``) and any-instance (``.../+/cmd/#``) inboxes are subscribed (D-U28); the
verb is extracted from the unambiguous ``/cmd/`` marker for either scope.
"""
import hashlib
import heapq
import json
import logging
import math
import re
import threading
import time
import unicodedata
import uuid
from concurrent.futures import ThreadPoolExecutor, wait
from collections import deque
from collections.abc import Mapping
from copy import deepcopy
from dataclasses import dataclass
from enum import Enum
from typing import (
    TYPE_CHECKING,
    Any,
    Callable,
    Deque,
    Dict,
    List,
    Optional,
    Set,
    Tuple,
)

from edgecommons.uns import Uns, UnsClass, UnsScope

if TYPE_CHECKING:
    from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity
    from edgecommons.messaging.message import Message

logger = logging.getLogger("CommandInbox")

#: The liveness/echo built-in verb.
PING = "ping"

#: The re-fetch/re-apply-configuration built-in verb.
RELOAD_CONFIG = "reload-config"

#: The descriptor discovery built-in verb.
DESCRIBE = "describe"

#: The return-my-redacted-effective-config built-in verb (Flow B).
GET_CONFIGURATION = "get-configuration"

#: The universal component status verb:
#: ``{"status": "RUNNING", "uptimeSecs": n[, "instances": [...]]}``.
#:
#: :data:`PING` answers only for the component as a whole. ``status`` is its per-instance
#: superset: it returns the same sample the ``state`` keepalive pushes in ``instances[]``,
#: sourced from the one component-supplied ``InstanceConnectivityProvider``. Push and pull
#: can therefore never disagree - a console can subscribe, or ask, and get the same answer.
#:
#: Every component implements it by registering that provider; a component with no
#: instances (a plain service) simply omits the section. It is deliberately **not** named
#: ``sb/status``: a processor or a sink has no southbound, and this verb is universal.
STATUS = "status"

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

#: Error code: a deferred command was sent without a guarded reply target.
ERR_REPLY_REQUIRED = "REPLY_REQUIRED"

#: Error code: bounded deferred-reply capacity is exhausted.
ERR_DEFERRED_REPLY_CAPACITY = "RESOURCE_LIMIT"

#: Error code attempted for open replies during component shutdown.
ERR_COMPONENT_STOPPING = "COMPONENT_STOPPING"

#: Hard bound on active provisional/open/settling deferred replies.
MAX_DEFERRED_REPLIES = 1024

#: Maximum inbox-owned post-accept continuations that may be running or queued.
MAX_POST_ACCEPT_CONTINUATIONS = 256

#: Camera-design upper bound (31 minutes) for one deferred reply lifetime.
MAX_DEFERRED_REPLY_LIFETIME_SECS = 31 * 60.0

#: Default bounded wait for MQTT SUBACK / Greengrass initial subscription response.
DEFAULT_START_TIMEOUT_SECS = 10.0

#: Strict bound for deliveries retained between transport acknowledgement and ACTIVE.
MAX_PENDING_STARTUP_DELIVERIES = 256

_MAX_START_ERROR_CHARS = 256

DEFERRED_REPLY_ATTEMPT_TIMEOUT_SECS = 5.0
DEFERRED_REPLY_RETRY_INITIAL_SECS = 0.1
DEFERRED_REPLY_RETRY_MAX_SECS = 1.0
DEFERRED_REPLY_SHUTDOWN_TIMEOUT_SECS = 1.0

#: The ``set-config`` push verb - delegated: the ``CONFIG_COMPONENT`` config source
#: maintains its own subscription for it on the same inbox path
#: (``ConfigComponentManager``), so the inbox must never dispatch or error-reply it.
SET_CONFIG_VERB = "set-config"

#: The built-in verbs (registered at construction; shadowing/unregistering is rejected).
BUILT_IN_VERBS = frozenset({PING, DESCRIBE, RELOAD_CONFIG, GET_CONFIGURATION, STATUS})

#: Verbs owned by other library subscriptions on the same inbox path - always ignored.
DELEGATED_VERBS = frozenset({SET_CONFIG_VERB})


class CommandInboxStartupState(str, Enum):
    """Observable lifecycle state of the command plane."""

    STARTING = "STARTING"
    ACTIVE = "ACTIVE"
    FAILED = "FAILED"
    STOPPED = "STOPPED"


@dataclass(frozen=True)
class CommandInboxStartupStatus:
    """Immutable lifecycle status; ``error`` is sanitized and bounded."""

    state: CommandInboxStartupState
    error: str = ""


@dataclass
class _ActivationGate:
    generation: int
    prefix: str
    pending: Deque[Tuple[Optional[str], Any]]
    retained: int = 0
    draining: bool = False

#: A command-verb handler: ``(request: Message) -> Optional[dict]``. The return value
#: is the verb-specific result object, wrapped by the inbox into the success reply
#: body; ``None`` yields an empty result (a plain acknowledgement). Raise
#: :class:`CommandException` for a coded error reply; any other exception becomes
#: :data:`ERR_HANDLER_ERROR`. Handlers run synchronously on the messaging delivery
#: thread - keep them fast, or hand off internally.
CommandHandler = Callable[["Message"], Optional[dict]]


class CommandOutcome:
    """Explicit outcome returned by a handler registered with ``register_outcome``."""

    @staticmethod
    def success(result: Optional[dict] = None) -> "ImmediateSuccess":
        return ImmediateSuccess(result)

    @staticmethod
    def error(code: str, message: Optional[str] = None) -> "ImmediateError":
        return ImmediateError(code, message)

    @staticmethod
    def deferred(
        token: "DeferredReply", post_accept_continuation: Optional[Callable[[], None]] = None
    ) -> "Deferred":
        """Returns a deferred result, optionally with inbox-owned post-accept work."""
        return Deferred(token, post_accept_continuation)

    @staticmethod
    def deferred_with_continuation(
        token: "DeferredReply", post_accept_continuation: Callable[[], None]
    ) -> "Deferred":
        """Starts the continuation only after this inbox accepts an OPEN token."""
        return Deferred(token, post_accept_continuation)


@dataclass(frozen=True)
class ImmediateSuccess(CommandOutcome):
    """Immediate standard success; ``None`` becomes an empty acknowledgement."""

    result: Optional[dict] = None


@dataclass(frozen=True)
class ImmediateError(CommandOutcome):
    """Immediate standard coded error."""

    code: str
    message: Optional[str] = None

    def __post_init__(self):
        if not self.code:
            raise ValueError("immediate error code must be non-empty")
        if self.message is None:
            object.__setattr__(self, "message", "")


@dataclass(frozen=True)
class Deferred(CommandOutcome):
    """An activated opaque reply handle that suppresses automatic reply.

    ``post_accept_continuation`` is started by the inbox only after it has validated this exact
    token in ``OPEN`` state. It must settle its captured token using the guarded API.
    """

    token: "DeferredReply"
    post_accept_continuation: Optional[Callable[[], None]] = None

    def __post_init__(self):
        if not isinstance(self.token, DeferredReply):
            raise ValueError("deferred token must be a DeferredReply")
        if self.post_accept_continuation is not None and not callable(
            self.post_accept_continuation
        ):
            raise ValueError("post-accept continuation must be callable")


OutcomeCommandHandler = Callable[["Message"], CommandOutcome]


class DeferredReplyState(Enum):
    PROVISIONAL = "PROVISIONAL"
    OPEN = "OPEN"
    SETTLING = "SETTLING"
    SETTLED = "SETTLED"
    DISCARDED = "DISCARDED"
    EXPIRED = "EXPIRED"
    CANCELLED_ON_SHUTDOWN = "CANCELLED_ON_SHUTDOWN"


class SettlementResult(Enum):
    ACCEPTED = "ACCEPTED"
    ALREADY_SETTLED = "ALREADY_SETTLED"
    EXPIRED = "EXPIRED"
    CANCELLED_ON_SHUTDOWN = "CANCELLED_ON_SHUTDOWN"
    NOT_OPEN = "NOT_OPEN"


@dataclass(frozen=True)
class DeferredReplySnapshot:
    capacity: int
    active: int
    provisioned: int
    settled: int
    discarded: int
    expired: int
    open_expired: int
    cancelled_on_shutdown: int
    capacity_rejected: int


@dataclass
class _DeferredEntry:
    id: uuid.UUID
    verb: str
    correlation_id: str
    reply_to: str
    request_uuid: Optional[str]
    request_metadata: "Message"
    expires_at: float
    state: DeferredReplyState = DeferredReplyState.PROVISIONAL
    activated: bool = False
    cleaned: bool = False
    attempts: int = 0
    reply: Optional["Message"] = None


class DeferredReply:
    """Opaque inbox-issued handle for one deferred command reply."""

    __slots__ = ("_owner", "_entry")

    def __init__(self, owner: "CommandInbox", entry: _DeferredEntry):
        self._owner = owner
        self._entry = entry

    def activate(self) -> bool:
        """Activates the token after the application durably accepts its work."""
        return self._owner._activate_deferred(self._entry)

    def discard(self) -> bool:
        """Discards a provisional token after durable acceptance fails."""
        return self._owner._discard_deferred(self._entry)

    def settle_success(self, result: Optional[dict] = None) -> SettlementResult:
        """Begins one success reply; exactly one concurrent caller is accepted."""
        return self._owner._settle_deferred(
            self._entry,
            {"ok": True, "result": result if result is not None else {}},
        )

    def settle_error(self, code: str, message: Optional[str] = None) -> SettlementResult:
        """Begins one coded error reply; exactly one concurrent caller is accepted."""
        if not code:
            raise ValueError("deferred error code must be non-empty")
        return self._owner._settle_deferred(
            self._entry, _error_body(code, message)
        )

    def state(self) -> DeferredReplyState:
        return self._owner._deferred_state(self._entry)

    def __repr__(self) -> str:
        return f"DeferredReply(opaque,state={self.state().value})"


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
    DESCRIBE = DESCRIBE
    RELOAD_CONFIG = RELOAD_CONFIG
    GET_CONFIGURATION = GET_CONFIGURATION
    STATUS = STATUS
    CMD_MESSAGE_VERSION = CMD_MESSAGE_VERSION
    ERR_UNKNOWN_VERB = ERR_UNKNOWN_VERB
    ERR_HANDLER_ERROR = ERR_HANDLER_ERROR
    ERR_RELOAD_FAILED = ERR_RELOAD_FAILED
    ERR_NO_CONFIG = ERR_NO_CONFIG
    ERR_REPLY_REQUIRED = ERR_REPLY_REQUIRED
    ERR_DEFERRED_REPLY_CAPACITY = ERR_DEFERRED_REPLY_CAPACITY
    ERR_COMPONENT_STOPPING = ERR_COMPONENT_STOPPING
    MAX_DEFERRED_REPLIES = MAX_DEFERRED_REPLIES
    MAX_POST_ACCEPT_CONTINUATIONS = MAX_POST_ACCEPT_CONTINUATIONS
    MAX_DEFERRED_REPLY_LIFETIME_SECS = MAX_DEFERRED_REPLY_LIFETIME_SECS
    DEFAULT_START_TIMEOUT_SECS = DEFAULT_START_TIMEOUT_SECS
    MAX_PENDING_STARTUP_DELIVERIES = MAX_PENDING_STARTUP_DELIVERIES
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
        instance_connectivity: Optional[
            Callable[[], Optional[List["InstanceConnectivity"]]]
        ] = None,
    ):
        """Creates the inbox and registers the built-in verbs. The verb
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
        :param instance_connectivity: the :data:`STATUS` source - the live per-instance
            connectivity sample (production:
            ``EnhancedHeartbeat.sample_instance_connectivity``, i.e. the very same provider
            the ``state`` keepalive pushes, so the pulled answer and the pushed one cannot
            diverge). ``None`` (the default) means "this component reports no instances",
            and ``status`` then answers exactly as ``ping`` does.
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
        if instance_connectivity is None:
            instance_connectivity = list  # no provider -> no instances[] section

        self._config_manager = config_manager
        self._messaging_client = messaging_client

        # verb -> handler; built-ins seeded here, custom verbs via register().
        self._handlers: Dict[str, CommandHandler] = {}
        self._outcome_handlers: Dict[str, OutcomeCommandHandler] = {}
        self._panels: Dict[str, dict] = {}

        # ping -> the state keepalive's RUNNING body shape: proves the component is
        # not just alive (the keepalive does that) but RESPONSIVE to addressed
        # commands.
        def _ping(request):
            return {"status": "RUNNING", "uptimeSecs": uptime_secs()}

        # status -> ping's per-instance superset. Same body, plus the instances[] the
        # state keepalive pushes, from the same provider. A component with no instances
        # omits the section, so a plain service answers exactly as ping does.
        def _status(request):
            result = {"status": "RUNNING", "uptimeSecs": uptime_secs()}
            conns = instance_connectivity()
            if conns:
                instances = [c.to_dict() for c in conns if c is not None]
                if instances:
                    result["instances"] = instances
            return result

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

        def _describe(request):
            commands = [
                {"verb": verb, "builtIn": verb in BUILT_IN_VERBS}
                for verb in sorted(self.verbs())
            ]
            panels = self._panel_descriptor()
            manifest = {
                "schemaVersion": "edgecommons.component.describe.v1",
                "commands": commands,
                "panels": panels,
            }
            component = self._component_descriptor()
            if component is not None:
                manifest["component"] = component
            manifest["digest"] = _describe_digest(commands, panels)
            return manifest

        # get-configuration (Flow B) -> the cfg class's body shape, as a reply.
        def _get_configuration(request):
            config = redacted_config()
            if config is None:
                raise CommandException(
                    ERR_NO_CONFIG, "no effective configuration is available"
                )
            return {"config": config}

        self._handlers[PING] = _ping
        self._handlers[STATUS] = _status
        self._handlers[DESCRIBE] = _describe
        self._handlers[RELOAD_CONFIG] = _reload_config
        self._handlers[GET_CONFIGURATION] = _get_configuration

        # The instance-scoped inbox filter (".../+/cmd/#", D-U28); None until start()
        # builds it.
        self._inbox_filter: Optional[str] = None
        # The component-scoped inbox filter (".../cmd/#", D-U28); None until start()
        # builds it.
        self._component_inbox_filter: Optional[str] = None
        # The instance filter minus the trailing '#' - the legacy verb-extraction prefix
        # (".../cmd/"); assigned BEFORE subscribing so a delivery racing the subscribe
        # call sees it. Verb extraction now uses the "/cmd/" marker (D-U28), so this is
        # retained only for the activation gate.
        self._inbox_prefix: Optional[str] = None

        self._lock = threading.RLock()
        self._deferred_lock = threading.RLock()
        self._deferred_condition = threading.Condition(self._deferred_lock)
        self._deferred_entries: Dict[uuid.UUID, _DeferredEntry] = {}
        self._deferred_expirations = []
        self._deferred_sequence = 0
        self._deferred_timer_stop = False
        self._deferred_timer_thread: Optional[threading.Thread] = None
        self._deferred_publishers = ThreadPoolExecutor(
            max_workers=32,
            thread_name_prefix="edgecommons-deferred-reply-publisher",
        )
        self._post_accept_slots = threading.BoundedSemaphore(
            MAX_POST_ACCEPT_CONTINUATIONS
        )
        self._post_accept_continuations = ThreadPoolExecutor(
            max_workers=4,
            thread_name_prefix="edgecommons-post-accept",
        )
        self._deferred_counters = {
            "provisioned": 0,
            "settled": 0,
            "discarded": 0,
            "expired": 0,
            "open_expired": 0,
            "cancelled_on_shutdown": 0,
            "capacity_rejected": 0,
        }
        self._startup_status = CommandInboxStartupStatus(
            CommandInboxStartupState.STOPPED
        )
        self._startup_generation = 0
        self._activation_gate: Optional[_ActivationGate] = None
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
        with self._lock:
            self._validate_custom_verb_registration(verb)
            self._handlers[verb] = handler
        logger.debug("Command verb '%s' registered", verb)

    def register_outcome(self, verb: str, handler: OutcomeCommandHandler) -> None:
        """Registers a handler that returns an explicit :class:`CommandOutcome`.

        This is additive: legacy :meth:`register` handlers keep their original
        ``dict``/``None`` and exception behavior.
        """
        if verb is None:
            raise ValueError("verb must not be None")
        if handler is None:
            raise ValueError("handler must not be None")
        for token in verb.split("/"):
            Uns.check_token(token, "verb token")
        with self._lock:
            self._validate_custom_verb_registration(verb)
            self._outcome_handlers[verb] = handler
        logger.debug("Outcome command verb '%s' registered", verb)

    def _validate_custom_verb_registration(self, verb: str) -> None:
        if verb in BUILT_IN_VERBS:
            raise ValueError(
                f"verb '{verb}' is a built-in verb and cannot be shadowed"
            )
        if verb in DELEGATED_VERBS:
            raise ValueError(
                f"verb '{verb}' is owned by another library subsystem and cannot"
                " be registered"
            )
        if verb in self._handlers or verb in self._outcome_handlers:
            raise ValueError(
                f"verb '{verb}' is already registered - unregister it first to"
                " replace the handler"
            )

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
        with self._lock:
            removed = self._handlers.pop(verb, None)
            outcome_removed = self._outcome_handlers.pop(verb, None)
        if removed is not None or outcome_removed is not None:
            logger.debug("Command verb '%s' unregistered", verb)

    def verbs(self) -> Set[str]:
        """The currently registered verbs (built-ins + custom) - a snapshot copy."""
        with self._lock:
            return set(self._handlers.keys()) | set(self._outcome_handlers.keys())

    def register_panel(self, panel: Mapping[str, Any]) -> None:
        """Registers a descriptor panel view for the built-in ``describe`` verb.

        The core library validates only the cross-language contract: the panel must
        be a JSON object with non-empty string ``id`` and ``title`` fields, and ids
        cannot be duplicated. All other descriptor fields are console-interpreted.
        """
        if not isinstance(panel, Mapping):
            raise ValueError("panel must be a JSON object")
        panel_id = panel.get("id")
        if not isinstance(panel_id, str) or not panel_id:
            raise ValueError("panel id must be a non-empty string")
        title = panel.get("title")
        if not isinstance(title, str) or not title:
            raise ValueError("panel title must be a non-empty string")
        if panel_id in self._panels:
            raise ValueError(f"panel id '{panel_id}' is already registered")
        self._panels[panel_id] = deepcopy(dict(panel))
        logger.debug("Command panel '%s' registered", panel_id)

    def panels(self) -> list:
        """The registered descriptor panel views - a snapshot copy."""
        return [deepcopy(panel) for panel in self._panels.values()]

    def _panel_descriptor(self) -> dict:
        views = self.panels()
        identity = self._safe_component_identity()
        panels = {
            "schemaVersion": "edgecommons.panels.v2",
            "provider": identity.component if identity is not None else "component",
            "renderer": "descriptor",
            "views": views,
        }
        if views:
            panels["defaultView"] = views[0]["id"]
        return panels

    def _component_descriptor(self) -> Optional[dict]:
        identity = self._safe_component_identity()
        if identity is None:
            return None
        return identity.to_dict()

    def _safe_component_identity(self):
        try:
            return self._config_manager.get_component_identity()
        except Exception:  # noqa: BLE001 - describe is best-effort discovery metadata
            return None

    # ----- deferred command replies -------------------------------------------------

    def defer(self, request: "Message", lifetime_secs: float) -> DeferredReply:
        """Creates a guarded provisional deferred-reply handle.

        The application must durably accept its work and then call
        :meth:`DeferredReply.activate`; on acceptance failure it calls ``discard``.
        No full request body or caller-controlled topic is exposed through the handle.
        """
        if request is None or request.get_header() is None:
            raise CommandException(ERR_REPLY_REQUIRED, "deferred command requires a request")
        if isinstance(lifetime_secs, bool) or not isinstance(lifetime_secs, (int, float)):
            raise TypeError("deferred reply lifetime_secs must be a number")
        lifetime = float(lifetime_secs)
        if not math.isfinite(lifetime) or lifetime <= 0:
            raise ValueError("deferred reply lifetime_secs must be finite and positive")
        if lifetime > MAX_DEFERRED_REPLY_LIFETIME_SECS:
            raise ValueError(
                "deferred reply lifetime_secs exceeds the 31-minute core maximum"
            )

        header = request.get_header()
        reply_to = header.reply_to
        if not reply_to:
            raise CommandException(
                ERR_REPLY_REQUIRED,
                "deferred command requires request/reply with a non-empty reply_to",
            )
        validator = getattr(self._messaging_client, "validate_reply_target", None)
        if validator is not None:
            validator(request)
        else:
            guard = getattr(self._messaging_client, "_check_reserved_topic", None)
            if guard is not None:
                guard(reply_to)
        if not header.name:
            raise ValueError("deferred request header.name must be non-empty")
        if not header.correlation_id:
            raise ValueError("deferred request correlation_id must be non-empty")

        from edgecommons.messaging.message_builder import MessageBuilder

        # Retain only the metadata necessary for guarded standard reply.  The command
        # body, tags, and caller identity are deliberately not retained.
        request_metadata = (
            MessageBuilder.create(header.name, CMD_MESSAGE_VERSION)
            .with_correlation_id(header.correlation_id)
            .with_reply_to(reply_to)
            .build()
        )
        entry = _DeferredEntry(
            id=uuid.uuid4(),
            verb=header.name,
            correlation_id=header.correlation_id,
            reply_to=reply_to,
            request_uuid=header.uuid,
            request_metadata=request_metadata,
            expires_at=time.monotonic() + lifetime,
        )
        with self._deferred_lock:
            if self._closed:
                raise CommandException(
                    ERR_COMPONENT_STOPPING,
                    "the component is stopping and cannot defer another reply",
                )
            if len(self._deferred_entries) >= MAX_DEFERRED_REPLIES:
                self._deferred_counters["capacity_rejected"] += 1
                raise CommandException(
                    ERR_DEFERRED_REPLY_CAPACITY,
                    "deferred reply capacity is exhausted",
                )
            self._deferred_entries[entry.id] = entry
            self._deferred_counters["provisioned"] += 1
            self._deferred_sequence += 1
            heapq.heappush(
                self._deferred_expirations,
                (entry.expires_at, self._deferred_sequence, "expire", entry),
            )
            if self._deferred_timer_thread is None:
                timer_thread = threading.Thread(
                    target=self._deferred_timer_loop,
                    name="edgecommons-deferred-reply-timer",
                    daemon=True,
                )
                self._deferred_timer_thread = timer_thread
                try:
                    timer_thread.start()
                except Exception:
                    self._deferred_timer_thread = None
                    entry.state = DeferredReplyState.DISCARDED
                    self._deferred_counters["discarded"] += 1
                    self._cleanup_deferred_locked(entry)
                    self._deferred_expirations = [
                        item
                        for item in self._deferred_expirations
                        if item[3] is not entry
                    ]
                    heapq.heapify(self._deferred_expirations)
                    raise
            self._deferred_condition.notify()
        return DeferredReply(self, entry)

    def _deferred_timer_loop(self) -> None:
        """Single inbox-owned timer for every deferred expiration."""
        while True:
            with self._deferred_condition:
                if self._deferred_timer_stop:
                    return
                while self._deferred_expirations:
                    scheduled_at, _, kind, entry = self._deferred_expirations[0]
                    if entry.cleaned or self._deferred_entries.get(entry.id) is not entry:
                        heapq.heappop(self._deferred_expirations)
                        continue
                    if kind == "retry" and entry.state is not DeferredReplyState.SETTLING:
                        heapq.heappop(self._deferred_expirations)
                        continue
                    remaining = scheduled_at - time.monotonic()
                    if remaining > 0:
                        self._deferred_condition.wait(timeout=remaining)
                        break
                    heapq.heappop(self._deferred_expirations)
                    if kind == "expire":
                        self._expire_deferred_locked(entry)
                    else:
                        try:
                            self._deferred_publishers.submit(
                                self._publish_deferred_attempt, entry
                            )
                        except RuntimeError:
                            # Executor shutdown races close(); close owns the terminal
                            # CANCELLED_ON_SHUTDOWN transition.
                            pass
                else:
                    self._deferred_condition.wait()

    def deferred_reply_snapshot(self) -> DeferredReplySnapshot:
        """Returns bounded-registry lifecycle counters for health and tests."""
        with self._deferred_lock:
            return DeferredReplySnapshot(
                capacity=MAX_DEFERRED_REPLIES,
                active=len(self._deferred_entries),
                provisioned=self._deferred_counters["provisioned"],
                settled=self._deferred_counters["settled"],
                discarded=self._deferred_counters["discarded"],
                expired=self._deferred_counters["expired"],
                open_expired=self._deferred_counters["open_expired"],
                cancelled_on_shutdown=self._deferred_counters[
                    "cancelled_on_shutdown"
                ],
                capacity_rejected=self._deferred_counters["capacity_rejected"],
            )

    def deferred_snapshot(self) -> DeferredReplySnapshot:
        """Compatibility alias for :meth:`deferred_reply_snapshot`."""
        return self.deferred_reply_snapshot()

    def _deferred_state(self, entry: _DeferredEntry) -> DeferredReplyState:
        with self._deferred_lock:
            return entry.state

    def _activate_deferred(self, entry: _DeferredEntry) -> bool:
        with self._deferred_lock:
            if self._deferred_entries.get(entry.id) is not entry:
                return False
            if entry.state is not DeferredReplyState.PROVISIONAL:
                return False
            entry.state = DeferredReplyState.OPEN
            entry.activated = True
            return True

    def _discard_deferred(self, entry: _DeferredEntry) -> bool:
        with self._deferred_lock:
            if self._deferred_entries.get(entry.id) is not entry:
                return False
            if entry.state is not DeferredReplyState.PROVISIONAL:
                return False
            entry.state = DeferredReplyState.DISCARDED
            self._deferred_counters["discarded"] += 1
            self._cleanup_deferred_locked(entry)
            return True

    def _settle_deferred(self, entry: _DeferredEntry, body: dict) -> SettlementResult:
        if not isinstance(body, dict):
            raise ValueError("deferred reply body must be a dict")
        with self._deferred_lock:
            if entry.state is not DeferredReplyState.OPEN:
                return self._settlement_result_for_state(entry.state)
        try:
            reply = self._build_deferred_reply(entry, deepcopy(body))
        except Exception as exc:  # noqa: BLE001 - leave OPEN so the caller may retry
            logger.warning(
                "Could not build deferred command reply for verb '%s': %s",
                entry.verb,
                exc,
            )
            return SettlementResult.NOT_OPEN
        with self._deferred_lock:
            state = entry.state
            if state is DeferredReplyState.OPEN:
                entry.state = DeferredReplyState.SETTLING
                entry.reply = reply
            else:
                return self._settlement_result_for_state(state)
        self._schedule_deferred_attempt(entry, 0.0)
        return SettlementResult.ACCEPTED

    @staticmethod
    def _settlement_result_for_state(state: DeferredReplyState) -> SettlementResult:
        if state in (DeferredReplyState.SETTLING, DeferredReplyState.SETTLED):
            return SettlementResult.ALREADY_SETTLED
        if state is DeferredReplyState.EXPIRED:
            return SettlementResult.EXPIRED
        if state is DeferredReplyState.CANCELLED_ON_SHUTDOWN:
            return SettlementResult.CANCELLED_ON_SHUTDOWN
        return SettlementResult.NOT_OPEN

    def _build_deferred_reply(self, entry: _DeferredEntry, body: dict) -> "Message":
        from edgecommons.messaging.message_builder import MessageBuilder

        return (
            MessageBuilder.create(entry.verb, CMD_MESSAGE_VERSION)
            .with_command(body)
            .with_config(self._config_manager)
            .build()
        )

    def _schedule_deferred_attempt(
        self, entry: _DeferredEntry, delay_secs: float
    ) -> None:
        with self._deferred_condition:
            if entry.state is not DeferredReplyState.SETTLING:
                return
            self._deferred_sequence += 1
            heapq.heappush(
                self._deferred_expirations,
                (
                    time.monotonic() + max(0.0, delay_secs),
                    self._deferred_sequence,
                    "retry",
                    entry,
                ),
            )
            self._deferred_condition.notify()

    def _publish_deferred_attempt(self, entry: _DeferredEntry) -> None:
        with self._deferred_lock:
            if entry.state is not DeferredReplyState.SETTLING:
                return
            remaining = entry.expires_at - time.monotonic()
            if remaining <= 0:
                self._expire_settling_deferred_locked(entry)
                return
            entry.attempts += 1
            attempt = entry.attempts
            reply = entry.reply
        try:
            self._messaging_client.reply_confirmed(
                entry.request_metadata,
                reply,
                min(DEFERRED_REPLY_ATTEMPT_TIMEOUT_SECS, remaining),
            )
        except Exception as exc:  # noqa: BLE001 - bounded retry until expiration
            with self._deferred_lock:
                if entry.state is not DeferredReplyState.SETTLING:
                    return
                remaining = entry.expires_at - time.monotonic()
                if remaining <= 0:
                    self._expire_settling_deferred_locked(entry)
                    return
            exponent = min(10, max(0, attempt - 1))
            delay = min(
                DEFERRED_REPLY_RETRY_MAX_SECS,
                DEFERRED_REPLY_RETRY_INITIAL_SECS * (2 ** exponent),
                remaining,
            )
            logger.debug(
                "Deferred reply attempt failed verb=%s request_uuid=%s attempt=%d;"
                " retrying in %.3fs: %s",
                entry.verb,
                entry.request_uuid,
                attempt,
                delay,
                exc,
            )
            self._schedule_deferred_attempt(entry, delay)
            return
        with self._deferred_lock:
            if entry.state is DeferredReplyState.SETTLING:
                entry.state = DeferredReplyState.SETTLED
                self._deferred_counters["settled"] += 1
                self._cleanup_deferred_locked(entry)

    def _expire_deferred(self, entry: _DeferredEntry) -> None:
        with self._deferred_lock:
            self._expire_deferred_locked(entry)

    def _expire_deferred_locked(self, entry: _DeferredEntry) -> None:
        if self._deferred_entries.get(entry.id) is not entry:
            return
        if entry.state is DeferredReplyState.SETTLING:
            # The in-flight strict operation has the same deadline. It owns the
            # SETTLED-vs-EXPIRED decision when the bounded wait returns.
            return
        if entry.state not in (
            DeferredReplyState.PROVISIONAL,
            DeferredReplyState.OPEN,
        ):
            return
        open_expiration = entry.activated
        prior_state = entry.state
        entry.state = DeferredReplyState.EXPIRED
        self._deferred_counters["expired"] += 1
        if open_expiration:
            self._deferred_counters["open_expired"] += 1
            logger.warning(
                "deferred_reply_expired verb=%s request_uuid=%s prior_state=%s attempts=%d",
                entry.verb,
                entry.request_uuid,
                prior_state.value,
                entry.attempts,
            )
        self._cleanup_deferred_locked(entry)

    def _expire_settling_deferred_locked(self, entry: _DeferredEntry) -> None:
        if (
            self._deferred_entries.get(entry.id) is not entry
            or entry.state is not DeferredReplyState.SETTLING
        ):
            return
        entry.state = DeferredReplyState.EXPIRED
        self._deferred_counters["expired"] += 1
        self._deferred_counters["open_expired"] += 1
        logger.warning(
            "deferred_reply_expired verb=%s request_uuid=%s prior_state=%s attempts=%d",
            entry.verb,
            entry.request_uuid,
            DeferredReplyState.SETTLING.value,
            entry.attempts,
        )
        self._cleanup_deferred_locked(entry)

    def _cleanup_deferred_locked(self, entry: _DeferredEntry) -> None:
        if entry.cleaned:
            return
        entry.cleaned = True
        if self._deferred_entries.get(entry.id) is entry:
            self._deferred_entries.pop(entry.id, None)

    def _accept_deferred_outcome(
        self, token: DeferredReply, verb: str, request: "Message"
    ) -> bool:
        if not isinstance(token, DeferredReply) or token._owner is not self:
            return False
        entry = token._entry
        header = request.get_header()
        with self._deferred_lock:
            return bool(
                entry.activated
                and entry.verb == verb
                and entry.reply_to == header.reply_to
                and entry.correlation_id == header.correlation_id
                and entry.request_uuid == header.uuid
                and entry.state
                in (
                    DeferredReplyState.OPEN,
                    DeferredReplyState.SETTLING,
                    DeferredReplyState.SETTLED,
                )
            )

    def start(
        self, timeout_secs: float = DEFAULT_START_TIMEOUT_SECS
    ) -> CommandInboxStartupStatus:
        """Start one acknowledged lifecycle generation.

        ``ACTIVE`` is published only after the selected transport proves MQTT SUBACK or
        Greengrass initial operation success. Deliveries racing activation are retained in
        strict arrival order by the bounded 256-message gate.
        """

        if isinstance(timeout_secs, bool) or not isinstance(timeout_secs, (int, float)):
            raise TypeError("command inbox start timeout_secs must be a number")
        timeout = float(timeout_secs)
        if not math.isfinite(timeout) or timeout <= 0:
            raise ValueError("command inbox start timeout_secs must be finite and positive")

        with self._lock:
            if self._closed:
                return CommandInboxStartupStatus(
                    CommandInboxStartupState.STOPPED, "command inbox is closed"
                )
            if self._startup_status.state in (
                CommandInboxStartupState.STARTING,
                CommandInboxStartupState.ACTIVE,
            ):
                return self._startup_status

            self._startup_generation += 1
            generation = self._startup_generation
            self._startup_status = CommandInboxStartupStatus(
                CommandInboxStartupState.STARTING
            )
            identity = self._config_manager.get_component_identity()
            if identity is None:
                self._fail_start_locked(generation, "no resolved component identity")
                logger.warning(
                    "No resolved component identity - the command inbox is disabled"
                )
                return self._startup_status
            try:
                uns = Uns(identity, self._config_manager.is_topic_include_root())
                site = identity.hier[0].value if len(identity.hier) >= 2 else None
                # D-U28: the component identity is component-scoped (no instance), so a
                # plain filter renders the instance slot as '+' (instance scope:
                # .../+/cmd/#); the component-scope filter omits the instance slot
                # (.../cmd/#). Subscribe both.
                scope = UnsScope(
                    site, identity.device, identity.component, identity.instance
                )
                filter_ = uns.filter(UnsClass.CMD, scope)
                component_filter = uns.filter(UnsClass.CMD, scope, include_instance=False)
                prefix = filter_[:-1]
                gate = _ActivationGate(generation, prefix, deque())
                self._inbox_filter = filter_
                self._component_inbox_filter = component_filter
                self._inbox_prefix = prefix
                self._activation_gate = gate
            except Exception as exc:
                self._fail_start_locked(generation, exc)
                return self._startup_status

        try:
            self._messaging_client.subscribe_acknowledged(
                filter_,
                lambda topic, message: self._receive_during_activation(
                    gate, topic, message
                ),
                None,
                10000,
                timeout,
            )
            self._messaging_client.subscribe_acknowledged(
                component_filter,
                lambda topic, message: self._receive_during_activation(
                    gate, topic, message
                ),
                None,
                10000,
                timeout,
            )
        except Exception as exc:  # noqa: BLE001 - failure is observable state
            self._unsubscribe_quietly(filter_)
            self._unsubscribe_quietly(component_filter)
            with self._lock:
                self._fail_start_locked(generation, exc)
                failed = self._startup_status
            logger.warning("Failed to start the command inbox: %s", failed.error)
            return failed

        stale = False
        with self._lock:
            stale = (
                self._closed
                or self._startup_generation != generation
                or self._startup_status.state
                is not CommandInboxStartupState.STARTING
            )
            if not stale:
                self._startup_status = CommandInboxStartupStatus(
                    CommandInboxStartupState.ACTIVE
                )
                if not gate.pending:
                    self._activation_gate = None
                else:
                    gate.draining = True
                    try:
                        threading.Thread(
                            target=self._drain_activation_gate,
                            args=(gate,),
                            name=f"edgecommons-command-activation-{generation}",
                            daemon=True,
                        ).start()
                    except Exception as exc:  # pragma: no cover - OS thread exhaustion
                        self._fail_start_locked(
                            generation,
                            f"command activation dispatcher rejected startup work: {exc}",
                        )
                        stale = True
                if not stale:
                    logger.info(
                        "Command inbox subscribed on '%s' and '%s' (verbs: %s)",
                        filter_,
                        component_filter,
                        sorted(self.verbs()),
                    )
        if stale:
            self._unsubscribe_quietly(filter_)
            self._unsubscribe_quietly(component_filter)
        return self.startup_status()

    def startup_status(self) -> CommandInboxStartupStatus:
        """Current observable lifecycle status."""

        with self._lock:
            return self._startup_status

    def stop(self) -> None:
        """Stop the current generation without permanently closing the inbox."""

        with self._lock:
            self._startup_generation += 1
            self._startup_status = CommandInboxStartupStatus(
                CommandInboxStartupState.STOPPED
            )
            filter_ = self._inbox_filter
            component_filter = self._component_inbox_filter
            self._inbox_filter = None
            self._component_inbox_filter = None
            self._inbox_prefix = None
            self._clear_activation_gate_locked()
        self._unsubscribe_quietly(filter_)
        self._unsubscribe_quietly(component_filter)

    def _fail_start_locked(self, generation: int, error: object) -> None:
        if (
            self._startup_generation != generation
            or self._startup_status.state is not CommandInboxStartupState.STARTING
        ):
            return
        self._inbox_filter = None
        self._component_inbox_filter = None
        self._inbox_prefix = None
        self._clear_activation_gate_locked()
        self._startup_status = CommandInboxStartupStatus(
            CommandInboxStartupState.FAILED, _sanitize_start_error(error)
        )

    def _clear_activation_gate_locked(self) -> None:
        gate = self._activation_gate
        if gate is not None:
            gate.pending.clear()
            gate.retained = 0
            gate.draining = False
            self._activation_gate = None

    def _unsubscribe_quietly(self, filter_: Optional[str]) -> None:
        if not filter_:
            return
        try:
            self._messaging_client.unsubscribe(filter_)
        except Exception as exc:  # noqa: BLE001 - best-effort cleanup
            logger.debug("Command-inbox unsubscribe of '%s' failed: %s", filter_, exc)

    def _receive_during_activation(
        self, gate: _ActivationGate, topic: Optional[str], message
    ) -> None:
        dispatch_now = False
        with self._lock:
            if self._closed or self._startup_generation != gate.generation:
                return
            state = self._startup_status.state
            if state is CommandInboxStartupState.STARTING or (
                state is CommandInboxStartupState.ACTIVE
                and self._activation_gate is gate
                and gate.draining
            ):
                if self._activation_gate is not gate:
                    return
                if gate.retained >= MAX_PENDING_STARTUP_DELIVERIES:
                    logger.warning(
                        "Dropping command delivery on '%s' because the bounded startup "
                        "activation queue is full (%d)",
                        topic,
                        MAX_PENDING_STARTUP_DELIVERIES,
                    )
                    return
                gate.pending.append((topic, message))
                gate.retained += 1
                return
            dispatch_now = state is CommandInboxStartupState.ACTIVE
        if dispatch_now:
            self._dispatch_delivery(gate.generation, gate.prefix, topic, message)

    def _drain_activation_gate(self, gate: _ActivationGate) -> None:
        while True:
            with self._lock:
                if (
                    self._closed
                    or self._startup_generation != gate.generation
                    or self._startup_status.state
                    is not CommandInboxStartupState.ACTIVE
                    or self._activation_gate is not gate
                ):
                    gate.pending.clear()
                    gate.retained = 0
                    return
                if not gate.pending:
                    gate.draining = False
                    self._activation_gate = None
                    return
                batch = list(gate.pending)
                gate.pending.clear()
            for topic, message in batch:
                try:
                    self._dispatch_delivery(
                        gate.generation, gate.prefix, topic, message
                    )
                finally:
                    with self._lock:
                        if gate.retained > 0:
                            gate.retained -= 1

    def _handle(self, topic: Optional[str], message) -> None:
        """Compatibility entry point for tests/manual delivery into the active generation."""

        with self._lock:
            generation = self._startup_generation
            prefix = self._inbox_prefix
        if prefix is not None:
            self._dispatch_delivery(generation, prefix, topic, message)

    def _dispatch_delivery(
        self, generation: int, prefix: str, topic: Optional[str], message
    ) -> None:
        """Validate and dispatch one delivery for exactly one active generation."""

        try:
            with self._lock:
                if (
                    self._closed
                    or self._startup_generation != generation
                    or self._startup_status.state
                    is not CommandInboxStartupState.ACTIVE
                ):
                    return
            # D-U28: the instance slot is optional, so a command arrives on either
            # ".../{instance}/cmd/{verb}" or ".../cmd/{verb}". Locate the "/cmd/" class
            # marker and take the verb after it - unambiguous for both scopes (an
            # instance is never a class token).
            if topic is None:
                return
            cmd_marker = topic.find("/cmd/")
            if cmd_marker < 0:
                # ".../cmd/#" also matches the bare ".../cmd" parent level - nothing
                # to dispatch.
                logger.debug("Ignoring cmd delivery without a '/cmd/' segment: '%s'", topic)
                return
            verb = topic[cmd_marker + 5:]   # 5 = len("/cmd/")
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
        with self._lock:
            handler = self._handlers.get(verb)
            outcome_handler = self._outcome_handlers.get(verb)
        if handler is None and outcome_handler is None:
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
        if outcome_handler is not None:
            self._dispatch_outcome(verb, request, outcome_handler, wants_reply)
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

    def _dispatch_outcome(
        self,
        verb: str,
        request: "Message",
        handler: OutcomeCommandHandler,
        wants_reply: bool,
    ) -> None:
        try:
            outcome = handler(request)
        except CommandException as exc:
            if wants_reply:
                self._send_reply(request, verb, _error_body(exc.code, exc.message))
            else:
                logger.warning(
                    "Fire-and-forget outcome verb '%s' failed (%s): %s",
                    verb,
                    exc.code,
                    exc.message,
                )
            return
        except Exception as exc:  # noqa: BLE001 - standard handler-error mapping
            if wants_reply:
                self._send_reply(
                    request, verb, _error_body(ERR_HANDLER_ERROR, str(exc))
                )
            else:
                logger.warning(
                    "Fire-and-forget outcome verb '%s' failed: %s", verb, exc
                )
            return

        if isinstance(outcome, ImmediateSuccess):
            if outcome.result is not None and not isinstance(outcome.result, dict):
                self._invalid_outcome(verb, request, wants_reply, "success result must be a dict")
                return
            if wants_reply:
                self._send_reply(
                    request,
                    verb,
                    {
                        "ok": True,
                        "result": outcome.result if outcome.result is not None else {},
                    },
                )
            return
        if isinstance(outcome, ImmediateError):
            if wants_reply:
                self._send_reply(
                    request, verb, _error_body(outcome.code, outcome.message)
                )
            else:
                logger.warning(
                    "Fire-and-forget outcome verb '%s' failed (%s): %s",
                    verb,
                    outcome.code,
                    outcome.message,
                )
            return
        if isinstance(outcome, Deferred):
            if self._accept_deferred_outcome(outcome.token, verb, request):
                if outcome.post_accept_continuation is not None:
                    # Existing Deferred outcomes keep their compatibility behavior. The
                    # post-accept form is stricter: application work begins only from OPEN.
                    if outcome.token.state() is not DeferredReplyState.OPEN:
                        self._invalid_outcome(
                            verb,
                            request,
                            wants_reply,
                            "post-accept continuation requires an open deferred token",
                        )
                        return
                    self._start_post_accept_continuation(
                        outcome.token, outcome.post_accept_continuation
                    )
                return
            # Do not leave an invalid still-provisional token occupying capacity.
            if outcome.token._owner is self:
                outcome.token.discard()
            self._invalid_outcome(
                verb,
                request,
                wants_reply,
                "deferred token must be activated and belong to this request",
            )
            return
        self._invalid_outcome(
            verb,
            request,
            wants_reply,
            "outcome handler must return ImmediateSuccess, ImmediateError, or Deferred",
        )

    def _start_post_accept_continuation(
        self, token: DeferredReply, continuation: Callable[[], None]
    ) -> None:
        """Starts bounded application work after exact OPEN-token acceptance.

        Queue rejection and uncaught continuation failures settle the token through the normal
        guarded error path. They never run application work on the command delivery thread or
        strand an accepted token without a response path.
        """
        if not self._post_accept_slots.acquire(blocking=False):
            logger.warning("Post-accept deferred continuation capacity exhausted")
            token.settle_error(
                ERR_HANDLER_ERROR,
                "the deferred command continuation could not be started",
            )
            return
        try:
            future = self._post_accept_continuations.submit(
                self._run_post_accept_continuation, token, continuation
            )
        except RuntimeError:
            self._post_accept_slots.release()
            logger.warning("Post-accept deferred continuation executor is unavailable")
            token.settle_error(
                ERR_HANDLER_ERROR,
                "the deferred command continuation could not be started",
            )
            return
        future.add_done_callback(lambda _completed: self._post_accept_slots.release())

    @staticmethod
    def _run_post_accept_continuation(
        token: DeferredReply, continuation: Callable[[], None]
    ) -> None:
        try:
            continuation()
        except Exception as exc:  # noqa: BLE001 - map application failure to standard reply
            logger.warning("Post-accept deferred continuation failed: %s", exc)
            token.settle_error(
                ERR_HANDLER_ERROR,
                "the deferred command continuation failed",
            )

    def _invalid_outcome(
        self, verb: str, request: "Message", wants_reply: bool, detail: str
    ) -> None:
        if wants_reply:
            self._send_reply(
                request,
                verb,
                _error_body(ERR_HANDLER_ERROR, f"invalid command outcome: {detail}"),
            )
        else:
            logger.warning(
                "Fire-and-forget outcome verb '%s' returned an invalid outcome: %s",
                verb,
                detail,
            )

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
            self._startup_generation += 1
            self._startup_status = CommandInboxStartupStatus(
                CommandInboxStartupState.STOPPED
            )
            inbox_filter = self._inbox_filter
            component_inbox_filter = self._component_inbox_filter
            self._inbox_filter = None
            self._component_inbox_filter = None
            self._inbox_prefix = None
            self._clear_activation_gate_locked()

        stopping_entries = []
        with self._deferred_condition:
            for entry in list(self._deferred_entries.values()):
                if entry.state is DeferredReplyState.OPEN:
                    # Claim the one reply capability for shutdown.  Settlement can
                    # no longer win, but cancellation is recorded only after the
                    # bounded COMPONENT_STOPPING attempt finishes.
                    entry.state = DeferredReplyState.SETTLING
                    stopping_entries.append(entry)
                elif entry.state in (
                    DeferredReplyState.PROVISIONAL,
                    DeferredReplyState.SETTLING,
                ):
                    entry.state = DeferredReplyState.CANCELLED_ON_SHUTDOWN
                    self._deferred_counters["cancelled_on_shutdown"] += 1
                    self._cleanup_deferred_locked(entry)
            self._deferred_timer_stop = True
            self._deferred_expirations.clear()
            timer_thread = self._deferred_timer_thread
            self._deferred_condition.notify_all()

        if timer_thread is not None and timer_thread is not threading.current_thread():
            timer_thread.join(timeout=0.2)
        self._deferred_publishers.shutdown(wait=False, cancel_futures=True)
        self._post_accept_continuations.shutdown(wait=False, cancel_futures=True)

        # Open tokens get one bounded COMPONENT_STOPPING attempt while messaging is
        # still alive.  A fixed worker bound prevents a 1,024-token shutdown from
        # becoming thread-per-reply; the overall wait is bounded too.
        if stopping_entries:
            executor = ThreadPoolExecutor(
                max_workers=min(32, len(stopping_entries)),
                thread_name_prefix="edgecommons-deferred-shutdown",
            )
            futures = [
                executor.submit(self._send_stopping_reply, entry)
                for entry in stopping_entries
            ]
            _, pending = wait(
                futures, timeout=DEFERRED_REPLY_SHUTDOWN_TIMEOUT_SECS
            )
            for future in pending:
                future.cancel()
            executor.shutdown(wait=False, cancel_futures=True)
            with self._deferred_lock:
                for entry in stopping_entries:
                    if entry.state is DeferredReplyState.SETTLING:
                        entry.state = DeferredReplyState.CANCELLED_ON_SHUTDOWN
                        self._deferred_counters["cancelled_on_shutdown"] += 1
                        self._cleanup_deferred_locked(entry)

        self._unsubscribe_quietly(inbox_filter)
        self._unsubscribe_quietly(component_inbox_filter)

    def _send_stopping_reply(self, entry: _DeferredEntry) -> None:
        try:
            reply = self._build_deferred_reply(
                entry,
                _error_body(
                    ERR_COMPONENT_STOPPING,
                    "the component stopped before the deferred command completed",
                ),
            )
            remaining = entry.expires_at - time.monotonic()
            if remaining > 0:
                self._messaging_client.reply_confirmed(
                    entry.request_metadata,
                    reply,
                    min(DEFERRED_REPLY_SHUTDOWN_TIMEOUT_SECS, remaining),
                )
        except Exception as exc:  # noqa: BLE001 - shutdown remains bounded/best effort
            logger.debug(
                "COMPONENT_STOPPING reply failed verb=%s request_uuid=%s: %s",
                entry.verb,
                entry.request_uuid,
                exc,
            )
        finally:
            with self._deferred_lock:
                if entry.state is DeferredReplyState.SETTLING:
                    entry.state = DeferredReplyState.CANCELLED_ON_SHUTDOWN
                    self._deferred_counters["cancelled_on_shutdown"] += 1
                    self._cleanup_deferred_locked(entry)


def _sanitize_start_error(error: object) -> str:
    source = "" if error is None else str(error)
    safe = []
    for char in source:
        if len(safe) >= _MAX_START_ERROR_CHARS:
            break
        safe.append(" " if unicodedata.category(char).startswith("C") else char)
    value = " ".join("".join(safe).split())
    value = re.sub(
        r"(?i)(password|passwd|token|secret)\s*[=:]\s*[^,; ]+",
        r"\1=***",
        value,
    )
    return re.sub(r"://[^/@ ]+@", "://***@", value)


def _error_body(code: str, message: Optional[str]) -> dict:
    """The error reply body ``{"ok": false, "error": {"code", "message"}}``."""
    return {"ok": False, "error": {"code": code, "message": message if message is not None else ""}}


def _describe_digest(commands: list, panels: dict) -> str:
    payload = json.dumps(
        {"commands": commands, "panels": panels},
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return "sha256:" + hashlib.sha256(payload).hexdigest()
