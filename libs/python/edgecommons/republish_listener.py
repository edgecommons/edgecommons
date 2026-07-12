"""The library-owned ``_bcast`` republish listener — the UNS "late-join lever"
(DESIGN-uns §9.3 layer 2 / §9.4, DESIGN-uns-bridge §2.5): every component subscribes,
on its PRIMARY (local/IPC) connection, the two per-device broadcast command topics for
its own device::

    ecv1/{device}/_bcast/main/cmd/republish-state
    ecv1/{device}/_bcast/main/cmd/republish-cfg

and, on receipt, re-announces out of band: ``republish-state`` re-emits the heartbeat's
``state`` keepalive (``{"status":"RUNNING","uptimeSecs":n}``) and ``republish-cfg``
re-runs the effective-config (``cfg``) publisher. Both re-announces go through the
privileged ``MessagingClient._publish_reserved*`` seam (via the injected actions),
which is why this is library plumbing — component code cannot publish the reserved
``state``/``cfg`` classes itself. The ``uns-bridge`` publishes these broadcasts on
every site-connection re-establishment so the site view rehydrates without broker
retain; the edge-console uses ``republish-cfg`` for config review.

Mirrors ``libs/java/.../uns/RepublishListener.java`` (the Java canonical); the
constants and wire contract are pinned by ``uns-test-vectors/bcast.json``.

**Normative behavior (identical across all four languages):**

- **Topics** — built through the topic builder with the reserved ``_bcast``
  pseudo-component identity: single-level hierarchy ``[{device: <own device>}]``,
  component :data:`BCAST_COMPONENT`, instance ``main``, class ``cmd``, channel = the
  verb. Always **rootless** (the identity is single-level, so ``includeRoot`` is a
  D-U25 no-op — the broadcast topic shape is device-bus-wide, independent of any
  component's own hierarchy/root mode).
- **Trigger validation** — the envelope's ``header.name`` must equal the topic's verb
  (:data:`REPUBLISH_STATE` / :data:`REPUBLISH_CFG`); the header ``version``, ``body``
  and any ``reply_to`` are ignored (fire-and-forget notification, no reply). A missing
  header, a mismatched name, or any parse anomaly is ignored (DEBUG log) — a malformed
  or foreign ``_bcast`` payload must never crash a component.
- **Jitter** — an accepted trigger fires after a uniformly random delay in
  ``[0, JITTER_WINDOW_MS]`` ms (the §9.3 "wait a random 0 to 2s" anti-stampede window: a
  whole fleet receives the broadcast at once). The randomness and clock are injected
  seams so the behavior unit-tests deterministically.
- **Coalescing / cooldown (per verb, independent)** — a trigger is accepted only when
  no re-announce for that verb is pending AND at least :data:`COOLDOWN_MS` ms have
  elapsed since the last *accepted* trigger for that verb (measured from acceptance,
  not from the jittered fire). Everything else coalesces into the pending/recent
  re-announce, so a looping or duplicated broadcast amplifies to at most one
  re-announce per verb per cooldown window.
- **No config surface** — always on; core plumbing, not a feature toggle. (The
  ``republish-state`` *action* still respects ``heartbeat.enabled``: a component whose
  operator disabled the state keepalive does not re-announce state.)

Lifecycle: constructed and :meth:`RepublishListener.start` started by the
``EdgeCommons`` runtime after initialization completes; :meth:`RepublishListener.close`
unsubscribes both topics (before messaging closes — the unsubscribe-before-exit rule)
and stops the jitter scheduler. When the component identity is not resolved (mock/test
bring-up), the listener disables itself with a WARN, mirroring the heartbeat and the
effective-config publisher.
"""
import logging
import random
import threading
import time
from typing import Callable, List, Optional

from edgecommons.messaging.identity import HierEntry, MessageIdentity
from edgecommons.uns import Uns, UnsClass

logger = logging.getLogger("RepublishListener")

#: The reserved broadcast pseudo-component token (UNS-CANONICAL-DESIGN §4.3).
BCAST_COMPONENT = "_bcast"

#: The re-announce-state broadcast verb (channel + envelope ``header.name``).
REPUBLISH_STATE = "republish-state"

#: The re-announce-effective-config broadcast verb (channel + envelope ``header.name``).
REPUBLISH_CFG = "republish-cfg"

#: The anti-stampede jitter window in ms: an accepted broadcast re-announces after a
#: uniformly random delay in ``[0, JITTER_WINDOW_MS]`` (DESIGN-uns §9.3: "a random 0 to
#: 2s"). Normative for all four languages.
JITTER_WINDOW_MS = 2_000

#: The per-verb coalescing cooldown in ms, measured from the last ACCEPTED trigger: at
#: most one re-announce per verb per this window, so a looping/duplicated broadcast
#: never amplifies. Normative for all four languages.
COOLDOWN_MS = 5_000


class _ThreadingDelayer:
    """The production delayer seam: schedules each call on its own daemon
    ``threading.Timer`` (Python's nearest analogue of Java's single-thread
    ``ScheduledExecutorService``). :meth:`close` best-effort cancels any timer that has
    not fired yet; a timer already mid-fire is unaffected — the listener's own
    ``closed`` check (taken under the same lock the fire path uses) makes that safe."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._timers: List[threading.Timer] = []

    def __call__(self, task: Callable[[], None], delay_millis: float) -> None:
        timer = threading.Timer(max(delay_millis, 0) / 1000.0, task)
        timer.daemon = True
        with self._lock:
            # Prune finished timers so a long-lived listener does not accumulate one
            # entry per broadcast forever.
            self._timers = [t for t in self._timers if t.is_alive()]
            self._timers.append(timer)
        timer.start()

    def close(self) -> None:
        with self._lock:
            timers, self._timers = self._timers, []
        for timer in timers:
            timer.cancel()


class _Command:
    """One broadcast verb's subscription + rate-limit state (guarded by the owning
    listener's lock)."""

    __slots__ = ("verb", "action", "topic", "pending", "triggered", "last_accepted_ms")

    def __init__(self, verb: str, action: Callable[[], None]):
        self.verb = verb
        self.action = action
        #: The resolved concrete topic; ``None`` until :meth:`RepublishListener.start`
        #: builds it.
        self.topic: Optional[str] = None
        #: A re-announce is scheduled but has not fired yet.
        self.pending = False
        #: Whether ``last_accepted_ms`` holds a real acceptance time.
        self.triggered = False
        #: Clock millis of the last ACCEPTED trigger (the cooldown reference point).
        self.last_accepted_ms = 0.0


def _default_jitter(window_ms: float) -> float:
    """The production jitter source: a uniformly random integer delay in
    ``[0, window_ms]`` (mirrors Java's ``ThreadLocalRandom.nextLong(window + 1)``)."""
    return random.randint(0, int(window_ms))


class RepublishListener:
    """See the module docstring for the full normative behavior."""

    #: Re-exported as class attributes for parity with the Java constants
    #: (``RepublishListener.BCAST_COMPONENT`` etc.) and for conformance tests.
    BCAST_COMPONENT = BCAST_COMPONENT
    REPUBLISH_STATE = REPUBLISH_STATE
    REPUBLISH_CFG = REPUBLISH_CFG
    JITTER_WINDOW_MS = JITTER_WINDOW_MS
    COOLDOWN_MS = COOLDOWN_MS

    def __init__(
        self,
        config_manager,
        messaging_client,
        state_republish: Callable[[], None],
        cfg_republish: Callable[[], None],
        delayer: Optional[Callable[[Callable[[], None], float], None]] = None,
        clock_millis: Optional[Callable[[], float]] = None,
        jitter: Optional[Callable[[float], float]] = None,
    ):
        """Production wiring (the three trailing params omitted): a daemon
        per-broadcast ``threading.Timer`` delayer, the wall clock, and a uniform
        random jitter.

        :param config_manager: the component's config manager (own-device identity
            resolution)
        :param messaging_client: the messaging handle (the ``MessagingClient`` class)
            whose PRIMARY connection carries the subscriptions
        :param state_republish: the ``republish-state`` action (the heartbeat's
            out-of-band state keepalive re-emit, e.g. ``EnhancedHeartbeat.publish_state_now``)
        :param cfg_republish: the ``republish-cfg`` action (the effective-config
            publisher's ``publish_now``)
        :param delayer: full-injection seam for deterministic tests: a callable
            ``(task, delay_millis) -> None``; ``None`` uses the production
            :class:`_ThreadingDelayer`
        :param clock_millis: full-injection seam for deterministic tests: a callable
            returning the current time in ms; ``None`` uses the wall clock
        :param jitter: full-injection seam for deterministic tests: a callable
            ``(window_ms) -> delay_millis``; ``None`` uses a uniform random jitter
        """
        if config_manager is None:
            raise ValueError("config_manager must not be None")
        if messaging_client is None:
            raise ValueError("messaging_client must not be None")
        if state_republish is None:
            raise ValueError("state_republish must not be None")
        if cfg_republish is None:
            raise ValueError("cfg_republish must not be None")

        self._config_manager = config_manager
        self._messaging_client = messaging_client
        self._commands: List[_Command] = [
            _Command(REPUBLISH_STATE, state_republish),
            _Command(REPUBLISH_CFG, cfg_republish),
        ]

        # Non-None only when this listener created (and therefore owns) the
        # production delayer, mirroring Java's `ownedScheduler`.
        self._owned_delayer: Optional[_ThreadingDelayer] = None
        if delayer is None:
            self._owned_delayer = _ThreadingDelayer()
            delayer = self._owned_delayer
        self._delayer = delayer
        self._clock_millis = clock_millis if clock_millis is not None else (
            lambda: time.time() * 1000.0
        )
        self._jitter = jitter if jitter is not None else _default_jitter

        self._lock = threading.RLock()
        self._started = False
        self._closed = False

    def start(self) -> None:
        """Builds the two own-device ``_bcast`` topics and subscribes them on the
        PRIMARY connection. Best-effort and idempotent: with no resolved component
        identity (mock/test bring-up) — or on any subscription failure — the listener
        logs and disables itself; the component must come up regardless."""
        with self._lock:
            if self._started or self._closed:
                return
            identity = self._config_manager.get_component_identity()
            if identity is None:
                logger.warning(
                    "No resolved component identity - the _bcast republish listener"
                    " is disabled"
                )
                return
            try:
                # The reserved _bcast pseudo-component pinned to this component's own
                # device. The identity is single-level, so the topic is rootless by
                # construction (D-U25) - the broadcast shape is shared by every
                # component on the device bus, whatever their own hierarchy/root mode.
                bcast_identity = MessageIdentity(
                    [HierEntry("device", identity.device)],
                    BCAST_COMPONENT,
                    MessageIdentity.DEFAULT_INSTANCE,
                )
                uns = Uns(bcast_identity, False)
                for command in self._commands:
                    topic = uns.topic(UnsClass.CMD, command.verb)
                    self._messaging_client.subscribe(
                        topic, lambda t, m, c=command: self._handle(c, m)
                    )
                    command.topic = topic
                self._started = True
                logger.info(
                    "Republish listener subscribed on '%s' and '%s'",
                    self._commands[0].topic,
                    self._commands[1].topic,
                )
            except Exception as e:  # noqa: BLE001 - best-effort by design
                logger.warning(
                    "Failed to start the _bcast republish listener (continuing"
                    " without it): %s",
                    e,
                )

    def _handle(self, command: _Command, message) -> None:
        """One received broadcast: validate the envelope (the ``header.name`` must
        equal the topic's verb), then run the accept/coalesce decision. Never raises —
        a malformed or foreign ``_bcast`` payload is ignored at DEBUG."""
        try:
            header = message.get_header() if message is not None else None
            if header is None or command.verb != header.name:
                logger.debug(
                    "Ignoring foreign/malformed _bcast payload on '%s'", command.topic
                )
                return
            self._on_broadcast(command)
        except Exception as e:  # noqa: BLE001 - a bad payload must never crash us
            logger.debug(
                "Ignoring malformed _bcast payload on '%s': %s", command.topic, e
            )

    def _on_broadcast(self, command: _Command) -> None:
        """The accept/coalesce decision (per verb): coalesce while a re-announce is
        pending or within :data:`COOLDOWN_MS` ms of the last accepted trigger;
        otherwise accept and schedule the re-announce after a jittered delay in
        ``[0, JITTER_WINDOW_MS]`` ms."""
        with self._lock:
            if self._closed:
                return
            now = self._clock_millis()
            if command.pending:
                logger.debug(
                    "'%s' broadcast coalesced (a re-announce is already pending)",
                    command.verb,
                )
                return
            if command.triggered and now - command.last_accepted_ms < COOLDOWN_MS:
                logger.debug(
                    "'%s' broadcast coalesced (within the %s ms cooldown)",
                    command.verb,
                    COOLDOWN_MS,
                )
                return
            command.pending = True
            command.triggered = True
            command.last_accepted_ms = now
            delay_millis = self._jitter(JITTER_WINDOW_MS)
        logger.debug(
            "'%s' broadcast accepted - re-announcing in %s ms", command.verb, delay_millis
        )
        self._delayer(lambda c=command: self._fire(c), delay_millis)

    def _fire(self, command: _Command) -> None:
        """The jittered re-announce: best-effort (a failing publisher must not kill
        the scheduler)."""
        with self._lock:
            command.pending = False
            if self._closed:
                return
        try:
            command.action()
        except Exception as e:  # noqa: BLE001 - best-effort by design
            logger.warning("'%s' re-announce failed: %s", command.verb, e)

    def close(self) -> None:
        """Stops the listener: unsubscribes both ``_bcast`` topics (while messaging is
        still up — the unsubscribe-before-exit rule), drops any pending re-announce,
        and shuts down the owned jitter scheduler. Idempotent."""
        with self._lock:
            if self._closed:
                return
            self._closed = True
            started = self._started
        if started:
            for command in self._commands:
                if command.topic is not None:
                    try:
                        self._messaging_client.unsubscribe(command.topic)
                    except Exception as e:  # noqa: BLE001 - best-effort by design
                        logger.debug(
                            "Republish-listener unsubscribe of '%s' failed: %s",
                            command.topic,
                            e,
                        )
        if self._owned_delayer is not None:
            self._owned_delayer.close()
