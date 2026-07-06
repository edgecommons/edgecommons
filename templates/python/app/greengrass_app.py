import logging
import math
import time
from abc import ABC

from edgecommons.command_inbox import CommandException
from edgecommons.config.manager.configuration_change_listener import (
    ConfigurationChangeListener,
)
from edgecommons.facades import Severity
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.metrics.metric_builder import MetricBuilder
from edgecommons.uns import UnsClass

logger = logging.getLogger("<<COMPONENTNAME>>")

# The demo loop-tick metric name (see the class docstring).
METRIC_NAME = "loopTicks"
# The demo data() signal id (see the class docstring).
DATA_SIGNAL_ID = "demo-signal"
# The custom command verb this scaffold registers (see the class docstring).
SET_GREETING = "set-greeting"


class <<COMPONENTNAME>>(ConfigurationChangeListener, ABC):
    """Minimal EdgeCommons component scaffold.

    The library gives you config, messaging, metrics, logging and lifecycle for free; the
    ``state`` heartbeat keepalive AND the component command inbox are both **automatic**
    (library-owned, no code here): the ``state`` keepalive publishes on
    ``ecv1/{device}/{component}/main/state`` (on / 5 s / local by default), and the inbox
    (``ecv1/{device}/{component}/main/cmd/#``, ``gg.get_commands()``) already answers ``ping`` /
    ``reload-config`` / ``get-configuration`` before ``run()`` is ever called.

    What this scaffold adds is the rest of the monitoring + command surface the edge-console
    reads (DESIGN-uns §7/§9 — G-S1/S2), so a freshly generated component has something to show
    up on the console's Signals/Events/Metrics tabs and something custom to command, instead of
    an empty dashboard:

    - a periodic **metric** (``loopTicks``: a monotonic ``tickCount`` counter plus an
      ``uptimeSecs`` gauge-like measure) via ``gg.get_metrics()``;
    - a periodic **data** signal (``demo-signal``: a sine-wave demo reading) via ``gg.data()`` —
      the ``DataFacade`` constructs the ``SouthboundSignalUpdate`` body (device/signal/samples)
      and defaults an omitted sample quality to ``GOOD``, so the console's Signals tab has
      something to chart;
    - a periodic **evt** (``ecv1/.../evt/info/sample-event``) via ``gg.events()`` — the
      ``EventsFacade`` derives the ``evt/{severity}/{type}`` channel from the body's own
      severity + type, so the topic and body can never disagree;
    - a custom **command verb** (``set-greeting``), registered with
      ``gg.get_commands().register(...)`` alongside the automatic built-ins, that mutates a
      small piece of in-memory state which the periodic status publish below then reflects on
      its very next tick — so invoking it from the console is visibly observable.

    Replace all four with your own business metrics/signals/events/verbs; none of this is
    required by the library (a bare scaffold works fine without them), it exists so the
    demonstrated surface is live end-to-end out of the box.
    """

    def __init__(self, gg):
        super().__init__()
        self._gg = gg
        self._config_manager = gg.get_config_manager()
        self._config_manager.add_config_change_listener(self)
        self._messaging = gg.get_messaging()
        self._metrics = gg.get_metrics()
        self._commands = gg.get_commands()
        # The data()/events() publish facades (DESIGN-class-facades.md) — bound to this
        # component's `main` instance, same as get_metrics()/get_commands() above.
        self._data = gg.data()
        self._events = gg.events()

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
        # (ecv1/{device}/{component}/{instance}/{class}/...). The data()/events() facades below
        # mint their OWN topics from the signal id / severity+type - only the `app` status
        # publish needs a hand-minted topic here.
        status_topic = self._gg.uns().topic(UnsClass.APP, "status")
        logger.info(
            "UNS identity path: %s - status=%s", self._gg.uns().identity().path, status_topic
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

            # 3) data - a periodic sample telemetry signal (the console's Signals tab), through
            # the data() facade: it constructs the SouthboundSignalUpdate body
            # (device/signal/samples), sanitizes the channel, and stamps identity - a real
            # adapter maps one protocol read onto add_sample(...) and never touches the envelope
            # or topic (DESIGN-class-facades §2.1). A sine wave stands in for a live sensor
            # reading here; the shorthand publish() with no explicit quality demonstrates the
            # facade's honest default - an unspecified reading defaults to Quality.GOOD (marked
            # qualityRaw="unspecified" on the wire so a consumer can tell a synthesized GOOD
            # from a device-reported one). Pass an explicit Quality.BAD/UNCERTAIN when your
            # source knows a read failed or is stale.
            demo_value = 20.0 + 5.0 * math.sin(seq / 10.0)
            self._data.publish(DATA_SIGNAL_ID, demo_value)

            # 4) evt - a discrete, human-meaningful occurrence (not a metric, not liveness
            # state); the console's Events tab. Through the events() facade: emit(type,
            # message, context, severity) derives the evt/{severity}/{type} channel from the
            # body's own severity + type, so the topic and body can never disagree
            # (DESIGN-class-facades §2.2) - no more hand-built body/topic. A real component
            # would emit these on actual occurrences (a threshold crossed, a connection
            # lost/restored, ...), not on a timer; raise_alarm/clear_alarm are there for
            # stateful alarms.
            self._events.emit(
                "sample-event",
                "sample event from <<COMPONENTNAME>>",
                {"seq": seq, "greeting": self._greeting},
                severity=Severity.INFO,
            )

            logger.info(
                "Running... (seq=%s uptimeSecs=%s greeting=%r)", seq, uptime_secs, self._greeting
            )
            time.sleep(10)
