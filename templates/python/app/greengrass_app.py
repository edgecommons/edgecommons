import logging
import time
from abc import ABC

from ggcommons.command_inbox import CommandException
from ggcommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.metrics.metric_builder import MetricBuilder
from ggcommons.uns import UnsClass

logger = logging.getLogger("<<COMPONENTNAME>>")

# The demo loop-tick metric name (see the class docstring).
METRIC_NAME = "loopTicks"
# The custom command verb this scaffold registers (see the class docstring).
SET_GREETING = "set-greeting"


class <<COMPONENTNAME>>(ConfigurationChangeListener, ABC):
    """Minimal GGCommons component scaffold.

    The library gives you config, messaging, metrics, logging and lifecycle for free; the
    ``state`` heartbeat keepalive AND the component command inbox are both **automatic**
    (library-owned, no code here): the ``state`` keepalive publishes on
    ``ecv1/{device}/{component}/main/state`` (on / 5 s / local by default), and the inbox
    (``ecv1/{device}/{component}/main/cmd/#``, ``gg.get_commands()``) already answers ``ping`` /
    ``reload-config`` / ``get-configuration`` before ``run()`` is ever called.

    What this scaffold adds is the rest of the monitoring + command surface the edge-console
    reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show
    up on the console's Events/Metrics tabs and something custom to command, instead of an empty
    dashboard:

    - a periodic **metric** (``loopTicks``: a monotonic ``tickCount`` counter plus an
      ``uptimeSecs`` gauge-like measure) via ``gg.get_metrics()``;
    - a periodic **evt** (``ecv1/.../evt/sample-event``) via the UNS topic builder +
      ``MessageBuilder`` — there is no dedicated ``events()`` facade yet, so an evt is just a
      normal published message on the open ``evt`` class;
    - a custom **command verb** (``set-greeting``), registered with
      ``gg.get_commands().register(...)`` alongside the automatic built-ins, that mutates a
      small piece of in-memory state which the periodic status publish below then reflects on
      its very next tick — so invoking it from the console is visibly observable.

    Replace all three with your own business metrics/events/verbs; none of this is required by
    the library (a bare scaffold works fine without them), it exists so the demonstrated surface
    is live end-to-end out of the box.
    """

    def __init__(self, gg):
        super().__init__()
        self._gg = gg
        self._config_manager = gg.get_config_manager()
        self._config_manager.add_config_change_listener(self)
        self._messaging = gg.get_messaging()
        self._metrics = gg.get_metrics()
        self._commands = gg.get_commands()

        # In-memory demo state: mutated by the set-greeting command, read back by the periodic
        # status publish in run() — so a console "Send command" has a visible effect without
        # needing a dedicated custom "get" verb (the built-in get-configuration already covers
        # reading config back).
        self._greeting = "Hello from <<COMPONENTNAME>>"

        # --- metrics: define once, emit every tick in run(). MetricBuilder is the sanctioned
        # construction path (never construct Metric directly). Two measures show a metric isn't
        # just a single scalar: a monotonic counter (tickCount) and a gauge-like elapsed value
        # (uptimeSecs); add_dimension adds a custom EMF/CloudWatch dimension on top of the
        # library's own default coreName/component dimensions.
        self._metrics.define_metric(
            MetricBuilder.create(METRIC_NAME)
            .with_config(self._config_manager)
            .add_measure("tickCount", "Count", 60)
            .add_measure("uptimeSecs", "Seconds", 60)
            .add_dimension("demo", "scaffold")
            .build()
        )

        # --- commands: ping/reload-config/get-configuration are already live (wired by the
        # library before __init__ runs). Register ONE custom verb so there is something for the
        # console's "Send command" to invoke beyond the built-ins. get_commands() is only None
        # on a mock/subclass bring-up that never initialized - guard defensively.
        if self._commands is not None:
            self._commands.register(SET_GREETING, self._handle_set_greeting)

    def _handle_set_greeting(self, request) -> dict:
        """The ``set-greeting`` custom command verb: ``{"greeting": "<new text>"}`` in,
        ``{"previousGreeting": ..., "greeting": ...}`` out. Raises :class:`CommandException`
        (a coded error reply, ``BAD_ARGS``) on a missing/malformed argument, exactly like the
        library's own built-ins do for their failure modes.

        Try it from the CLI (fire-and-forget doesn't need a reply_to; the inbox still runs the
        handler): publish ``{"header":{"name":"set-greeting","version":"1.0"},"body":
        {"greeting":"Hi from mqttx"}}`` to ``ecv1/{device}/{component}/main/cmd/set-greeting``.
        """
        body = request.get_body()
        if not isinstance(body, dict) or not isinstance(body.get("greeting"), str):
            raise CommandException("BAD_ARGS", 'expected a JSON body {"greeting": "<text>"}')
        previous = self._greeting
        self._greeting = body["greeting"]
        return {"previousGreeting": previous, "greeting": self._greeting}

    def on_configuration_change(self, configuration) -> bool:
        logger.info("Configuration changed.  Ignoring.")
        return True

    def run(self):
        # Mint every topic you publish or subscribe through the UNS topic builder — never
        # hand-write one. Topics carry the component's config-resolved identity
        # (ecv1/{device}/{component}/{instance}/{class}/...).
        status_topic = self._gg.uns().topic(UnsClass.APP, "status")
        event_topic = self._gg.uns().topic(UnsClass.EVT, "sample-event")
        logger.info(
            "UNS identity path: %s - status=%s event=%s",
            self._gg.uns().identity().path,
            status_topic,
            event_topic,
        )

        seq = 0
        start = time.monotonic()
        while True:
            seq += 1
            uptime_secs = int(time.monotonic() - start)

            # 1) app status - reflects the current greeting (mutable via the set-greeting
            # command above), so a console operator can watch a command's effect land.
            status_body = {"seq": seq, "message": self._greeting}
            status_msg = (
                MessageBuilder.create("StatusUpdate", "1.0")
                .with_payload(status_body)
                .with_config(self._config_manager)
                .build()
            )
            self._messaging.publish(status_topic, status_msg)

            # 2) metric - a loop-tick counter plus an uptime-ish gauge (the console's Metrics tab).
            self._metrics.emit_metric(
                METRIC_NAME, {"tickCount": float(seq), "uptimeSecs": float(uptime_secs)}
            )

            # 3) evt - a discrete, human-meaningful occurrence (not a metric, not liveness
            # state); the console's Events tab. A real component would emit these on actual
            # occurrences (a threshold crossed, a connection lost/restored, ...), not on a timer.
            event_body = {
                "severity": "info",
                "message": "sample event from <<COMPONENTNAME>>",
                "context": {"seq": seq, "greeting": self._greeting},
            }
            event_msg = (
                MessageBuilder.create("SampleEvent", "1.0")
                .with_payload(event_body)
                .with_config(self._config_manager)
                .build()
            )
            self._messaging.publish(event_topic, event_msg)

            logger.info(
                "Running... (seq=%s uptimeSecs=%s greeting=%r)", seq, uptime_secs, self._greeting
            )
            time.sleep(10)
