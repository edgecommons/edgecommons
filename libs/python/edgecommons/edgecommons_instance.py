"""The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped handle
whose only job is to pre-bind the instance token into (a) the
:class:`~edgecommons.uns.Uns` topic builder, (b) the
:class:`~edgecommons.messaging.message_builder.MessageBuilder`, and (c) the app-usable
publish facades (``data()``/``events()``/``app()`` — DESIGN-class-facades §3). The
messaging client stays instance-agnostic — ``publish(topic, msg)`` already receives both
the topic (minted by this handle's instance-bound ``uns()``) and the envelope (stamped by
its instance-bound builder), which is why the seam works unchanged over Python's
static/process-global ``MessagingClient``.

Obtain instance-scoped handles from ``EdgeCommons.instance(id)`` (validated + cached
per id). The library also builds one handle with ``instance_id=None`` for **component
scope** (D-U28): its ``uns()``/``new_message()``/facades omit the instance token
entirely, backing ``gg.data()``/``gg.events()``/``gg.app()``.
"""
from typing import Callable, Optional

from edgecommons.facades.app_facade import AppFacade
from edgecommons.facades.data_facade import DataFacade
from edgecommons.facades.events_facade import EventsFacade
from edgecommons.facades.stream_sink import StreamSink
from edgecommons.messaging.message_builder import MessageBuilder
from edgecommons.uns import Uns


class EdgeCommonsInstance:
    """An instance-scoped handle: ``uns()`` mints topics with — and ``new_message()``
    stamps envelopes with — this handle's instance token; ``data()``/``events()``/
    ``app()`` are the bound publish facades (DESIGN-class-facades §2)."""

    def __init__(self, instance_id: str, config_manager, include_root: bool,
                 messaging_client=None, stream_sink: Optional[StreamSink] = None,
                 clock: Optional[Callable] = None):
        """Library-internal: created by ``EdgeCommons.instance(id)``, which validates the
        token (§2.2 token rule) and caches per id, or by ``EdgeCommons._component_scope()``
        with ``instance_id=None`` for component scope (D-U28).

        :param instance_id: the instance token, or ``None`` for component scope (D-U28)
        :param config_manager: the component config manager
        :param include_root: the resolved ``topic.includeRoot`` mode
        :param messaging_client: the (guarded) messaging handle the facades publish
            through (the ``MessagingClient`` class); ``None`` only for callers that never
            touch ``data()``/``events()``/``app()`` (kept optional for backward
            compatibility with existing ``EdgeCommonsInstance(...)`` call sites/tests)
        :param stream_sink: the stream seam for ``data().via(Channel.stream(...))``, or
            ``None`` when streaming is not configured (a stream route then falls back to
            local)
        :param clock: a zero-arg callable returning the current timezone-aware
            ``datetime`` for the facades' time defaults (injected for deterministic
            tests); defaults to ``datetime.now(timezone.utc)`` inside each facade
        """
        self._id = instance_id
        self._config_manager = config_manager
        self._uns = Uns(
            config_manager.get_component_identity().with_instance(instance_id),
            include_root,
        )
        self._messaging_client = messaging_client
        self._stream_sink = stream_sink
        self._clock = clock

        # Lazily-created facades (per-instance; the facades hold no per-instance client
        # state beyond what is passed in here).
        self._data: Optional[DataFacade] = None
        self._events: Optional[EventsFacade] = None
        self._app: Optional[AppFacade] = None

    def id(self) -> str:
        """This handle's instance token."""
        return self._id

    def uns(self) -> Uns:
        """The topic builder bound to this instance (topics minted with this instance
        token)."""
        return self._uns

    def new_message(self, name: str, version: str) -> MessageBuilder:
        """Starts a message pre-bound to this instance — equivalent to
        ``MessageBuilder.create(name, version).with_config(config).with_instance(id())``,
        so ``build()`` stamps the component identity with this handle's instance
        token."""
        return (
            MessageBuilder.create(name, version)
            .with_config(self._config_manager)
            .with_instance(self._id)
        )

    def data(self) -> DataFacade:
        """The ``data()`` publish facade bound to this instance (DESIGN-class-facades
        §2.1): builds + validates the ``SouthboundSignalUpdate`` body (quality ->
        ``GOOD``, ``serverTs`` -> now, samples wrapper), sanitizes the signal path into
        the ``data`` channel, and routes on the resolved channel (per-call -> config
        ``publish.channel`` -> LOCAL).

        :raises RuntimeError: when this handle was constructed without a
            ``messaging_client`` (no publish surface to bind to)
        """
        if self._data is None:
            self._data = DataFacade(
                self._config_manager, self._id, self._uns,
                self._require_messaging_client(), self._stream_sink, self._clock,
            )
        return self._data

    def events(self) -> EventsFacade:
        """The ``events()`` publish facade bound to this instance (DESIGN-class-facades
        §2.2): operator events & alarms on the ``evt`` class, deriving the
        ``evt/{severity}/{type}`` channel from the body.

        :raises RuntimeError: when this handle was constructed without a
            ``messaging_client``
        """
        if self._events is None:
            self._events = EventsFacade(
                self._config_manager, self._id, self._uns,
                self._require_messaging_client(), self._clock,
            )
        return self._events

    def app(self) -> AppFacade:
        """The ``app()`` publish facade bound to this instance (DESIGN-class-facades
        §2.3): free-form inter-component pub/sub on the ``app`` class.

        :raises RuntimeError: when this handle was constructed without a
            ``messaging_client``
        """
        if self._app is None:
            self._app = AppFacade(
                self._config_manager, self._id, self._uns, self._require_messaging_client(),
            )
        return self._app

    def _require_messaging_client(self):
        if self._messaging_client is None:
            raise RuntimeError(
                "this EdgeCommonsInstance was constructed without a messaging_client - the"
                " data()/events()/app() publish facades have no publish surface to bind"
                " to"
            )
        return self._messaging_client
