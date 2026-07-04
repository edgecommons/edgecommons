"""The constructed ``SouthboundSignalUpdate`` body (DESIGN-class-facades §2.1,
``docs/SOUTHBOUND.md`` §2) -- the value object that **replaces the adapters' hand-
assembled dict**. :class:`SignalUpdate` holds the raw inputs (optional ``device`` block;
the signal with its REQUIRED stable ``signal_id`` plus optional ``signal_name``/
``signal_address``; the ``samples``; the sanitized-into-a-channel ``signal_path``; an
optional per-call :class:`~ggcommons.facades.channel.Channel` override).
:meth:`~ggcommons.facades.data_facade.DataFacade.build_body` applies the defaulting
rules (quality -> ``GOOD``, ``serverTs`` -> now, the ``samples`` wrapper) and produces
the wire body.

Obtain a builder from :meth:`~ggcommons.facades.data_facade.DataFacade.signal` and
terminate with :meth:`SignalUpdateBuilder.publish` (or :meth:`SignalUpdateBuilder.build`
for the :meth:`~ggcommons.facades.data_facade.DataFacade.publish_update` form). The
``signal_id`` is the only structural requirement -- a missing one is a fail-fast
:class:`ValueError` at publish (DESIGN-class-facades §5.2), never a dropped message.

Mirrors Java's ``SignalUpdate``/``SignalUpdate.Builder``
(``com.mbreissi.ggcommons.facades.SignalUpdate``); Python collapses the Java ``Sample.of``
overload set into one factory with keyword defaults (a Python-idiom divergence, not a
behavior difference).
"""
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, TYPE_CHECKING

from ggcommons.facades.channel import Channel
from ggcommons.facades.quality import Quality

if TYPE_CHECKING:
    from ggcommons.facades.data_facade import DataFacade


@dataclass(frozen=True)
class Sample:
    """One sample: a measured ``value`` (REQUIRED) plus the optional quality/timestamp
    parts. A ``None`` ``quality`` is defaulted to :attr:`Quality.GOOD` by the facade; a
    ``None`` ``server_ts`` is filled with now; ``source_ts`` is never synthesized;
    ``quality_raw`` is a synthetic ``"unspecified"`` marker when (and only when) the
    quality was defaulted, else passed through verbatim.

    :param value: the measured value (any JSON-native type) -- REQUIRED (``None`` is a
        fail-fast error at build)
    :param quality: the normalized quality, or ``None`` to default to :attr:`Quality.GOOD`
    :param quality_raw: the native status code, or ``None``
    :param source_ts: the device/field ISO-8601 timestamp, or ``None`` (never synthesized)
    :param server_ts: the protocol-server ISO-8601 timestamp, or ``None`` to default to now
    """

    value: Any
    quality: Optional[Quality] = None
    quality_raw: Optional[str] = None
    source_ts: Optional[str] = None
    server_ts: Optional[str] = None

    @staticmethod
    def of(value: Any, quality: Optional[Quality] = None,
           source_ts: Optional[str] = None) -> "Sample":
        """A value(+quality)(+device timestamp) sample; ``server_ts`` always defaults to
        now. Collapses Java's three ``Sample.of`` overloads into one factory with keyword
        defaults."""
        return Sample(value, quality, None, source_ts, None)


class SignalUpdate:
    """The immutable constructed signal update -- see the module docstring."""

    def __init__(self, device: Optional[Dict[str, Any]], signal_id: Optional[str],
                 signal_name: Optional[str], signal_address: Optional[Dict[str, Any]],
                 samples: List[Sample], signal_path: Optional[str],
                 via: Optional[Channel]):
        self.device = device
        self.signal_id = signal_id
        self.signal_name = signal_name
        self.signal_address = signal_address
        self.samples: List[Sample] = list(samples)
        self.signal_path = signal_path
        self.via = via

    @property
    def effective_signal_path(self) -> Optional[str]:
        """The effective channel path: :attr:`signal_path` when set, else :attr:`signal_id`."""
        return self.signal_path if self.signal_path is not None else self.signal_id


class SignalUpdateBuilder:
    """The fluent ``SouthboundSignalUpdate`` builder --
    ``signal(id).name(n).address(a).device(...).add_sample(...).signal_path(p).publish()``.

    :param signal_id: the stable ``signal.id`` (REQUIRED -- the consumer key)
    :param facade: the originating :class:`~ggcommons.facades.data_facade.DataFacade`
        (``None`` for a **detached** builder -- terminate with :meth:`build` and pass the
        result to :meth:`~ggcommons.facades.data_facade.DataFacade.publish_update`)
    """

    def __init__(self, signal_id: Optional[str], facade: Optional["DataFacade"] = None):
        self._facade = facade
        self._signal_id = signal_id
        self._device: Optional[Dict[str, Any]] = None
        self._signal_name: Optional[str] = None
        self._signal_address: Optional[Dict[str, Any]] = None
        self._samples: List[Sample] = []
        self._signal_path: Optional[str] = None
        self._via: Optional[Channel] = None

    def name(self, name: str) -> "SignalUpdateBuilder":
        """Sets the human ``signal.name``."""
        self._signal_name = name
        return self

    def address(self, address: Dict[str, Any]) -> "SignalUpdateBuilder":
        """Sets the protocol-native ``signal.address``."""
        self._signal_address = address
        return self

    def device(self, adapter: Optional[str] = None, instance: Optional[str] = None,
               endpoint: Optional[str] = None,
               block: Optional[Dict[str, Any]] = None) -> "SignalUpdateBuilder":
        """Sets the ``device`` block: either a pre-built ``block`` dict, or assembled from
        its three parts (any may be ``None``/omitted)."""
        if block is not None:
            self._device = block
            return self
        d: Dict[str, Any] = {}
        if adapter is not None:
            d["adapter"] = adapter
        if instance is not None:
            d["instance"] = instance
        if endpoint is not None:
            d["endpoint"] = endpoint
        self._device = d
        return self

    def add_sample(self, value_or_sample: Any, quality: Optional[Quality] = None,
                   source_ts: Optional[str] = None) -> "SignalUpdateBuilder":
        """Appends a sample: either a pre-built :class:`Sample` (sole argument), or a raw
        value (+ optional quality / device timestamp) -- ``quality``/``source_ts``
        default to ``None`` (quality -> ``GOOD``, ``server_ts`` -> now, per the facade's
        defaulting rules)."""
        if isinstance(value_or_sample, Sample):
            self._samples.append(value_or_sample)
        else:
            self._samples.append(Sample(value_or_sample, quality, None, source_ts, None))
        return self

    def add_samples(self, samples) -> "SignalUpdateBuilder":
        """Appends a batch of :class:`Sample` (the coalesced-publish path)."""
        self._samples.extend(samples)
        return self

    def signal_path(self, signal_path: str) -> "SignalUpdateBuilder":
        """Sets the channel path -- the ``data/{signal_path}`` tail (each ``/``-separated
        token is sanitized into a UNS token by the facade). When unset, the stable
        ``signal_id`` is used as the path (D-U15's sanitized-path-vs-stable-id split still
        holds -- the body's raw id rides untouched)."""
        self._signal_path = signal_path
        return self

    def via(self, channel: Channel) -> "SignalUpdateBuilder":
        """Sets a per-call :class:`~ggcommons.facades.channel.Channel` override
        (LOCAL / NORTHBOUND / stream)."""
        self._via = channel
        return self

    def build(self) -> SignalUpdate:
        """Builds the immutable :class:`SignalUpdate` (for the
        :meth:`~ggcommons.facades.data_facade.DataFacade.publish_update` form)."""
        return SignalUpdate(
            device=self._device,
            signal_id=self._signal_id,
            signal_name=self._signal_name,
            signal_address=self._signal_address,
            samples=self._samples,
            signal_path=self._signal_path,
            via=self._via,
        )

    def publish(self) -> None:
        """Builds and publishes through the originating facade.

        :raises RuntimeError: if this builder was created detached (no facade) -- use
            :meth:`build` + ``DataFacade.publish_update(...)``
        """
        if self._facade is None:
            raise RuntimeError(
                "this SignalUpdateBuilder is detached - call build() and pass it to"
                " DataFacade.publish_update(SignalUpdate)"
            )
        self._facade.publish_update(self.build())
