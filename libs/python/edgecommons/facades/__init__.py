"""The app-usable class publish facades: ``data()``/``events()``/``app()``
(``docs/platform/DESIGN-class-facades.md``). These are the **non-reserved** siblings of
the reserved publishers (heartbeat/``state``, ``MetricEmitter``/``metric``,
``EffectiveConfigPublisher``/``cfg``): they publish through the ordinary, guarded
``MessagingClient.publish(...)`` and add body-contract enforcement + defaults, not
privilege.

- :class:`~edgecommons.facades.data_facade.DataFacade` -- the ``data`` class (the
  telemetry/signal data plane): constructs + validates the ``SouthboundSignalUpdate``
  body (device/signal/samples), defaults ``quality`` to :attr:`Quality.GOOD` and
  ``serverTs`` to now, and routes on the resolved :class:`Channel`.
- :class:`~edgecommons.facades.events_facade.EventsFacade` -- the ``evt`` class (operator
  events & alarms): derives the ``evt/{severity}/{type}`` channel from the body.
- :class:`~edgecommons.facades.app_facade.AppFacade` -- the ``app`` class (free-form
  inter-component pub/sub): a named header + verbatim body.

Obtain bound instances from ``gg.instance(id).data()/events()/app()`` (primary,
per-instance) or the ``main``-instance convenience ``gg.data()/events()/app()``. Mirrors
the Java canonical ``com.mbreissi.edgecommons.facades`` package.
"""
from .app_facade import AppFacade, PreparedAppMessage
from .channel import Channel
from .data_facade import DataFacade
from .events_facade import EventsFacade
from .quality import Quality
from .severity import Severity
from .signal_update import Sample, SignalUpdate, SignalUpdateBuilder
from .stream_sink import StreamSink

__all__ = [
    "AppFacade",
    "PreparedAppMessage",
    "Channel",
    "DataFacade",
    "EventsFacade",
    "Quality",
    "Sample",
    "Severity",
    "SignalUpdate",
    "SignalUpdateBuilder",
    "StreamSink",
]
