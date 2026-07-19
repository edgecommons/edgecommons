"""The device seam: what a *protocol adapter* talks to.

:class:`DeviceSession` is one live connection to one device. Implement it once per protocol —
Modbus, OPC UA, whatever you are bridging — and everything above it (the connection lifecycle,
backoff, publishing, health) is written against the seam and never learns your protocol.

**The boundary rule, and it is worth enforcing in review:** a backend knows protocols. It does
**not** know EdgeCommons topics, the UNS, message envelopes, or metrics. If your ``DeviceSession``
imports ``edgecommons.uns``, the seam has leaked. That is why this module imports nothing from
``edgecommons``: the mapping from a protocol :class:`Reading` to a ``SouthboundSignalUpdate`` lives
one layer up, in ``adapter.py``.

## Signals, not tags

A **signal** is one data point — a measured value with identity, quality, and timestamps. (OPC UA
calls it a "tag"; Modbus calls it a "register".) The word "tag" is reserved in EdgeCommons for the
envelope's *business metadata*, which is a different thing entirely.

## Quality is not optional

Every sample carries a ``quality`` normalized to ``GOOD | BAD | UNCERTAIN``, plus the native code in
``quality_raw`` for diagnosis. This is what lets a consumer gate on quality without knowing your
protocol — and it is why a read failure is published as a ``BAD`` sample rather than swallowed: a
signal that silently stops updating is indistinguishable from one that is simply not changing.
"""
import math
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, Dict, List, Optional


# =================================================================================================
# Quality + value objects
# =================================================================================================

class Quality:
    """The normalized, protocol-independent sample-quality tokens. The protocol's own status code
    goes in :attr:`Reading.quality_raw`. ``UNCERTAIN`` is unused by the simulated backend and used
    constantly by real ones: a stale cached read, a value outside its calibrated range, a sensor
    that answered but warned."""

    GOOD = "GOOD"
    BAD = "BAD"
    UNCERTAIN = "UNCERTAIN"


@dataclass
class Reading:
    """One reading from the device."""

    #: The canonical, stable id the rest of the fleet keys on (e.g. ``ns=3;i=1001``).
    signal_id: str
    #: A human label, when the backend has one.
    name: Optional[str]
    value: Any
    #: One of :class:`Quality`'s tokens.
    quality: str
    #: The protocol-native status code, kept verbatim for diagnosis.
    quality_raw: Optional[str] = None


@dataclass
class SignalInfo:
    """One signal in the adapter's inventory — its stable id and human label, known from
    config/backend **without a device round-trip**. Backs the ``sb/signals`` command."""

    #: The canonical, stable id (the ``sb/read``/``sb/write`` ``signalId``).
    id: str
    #: A human label, when the backend has one.
    name: Optional[str] = None


@dataclass
class BrowsedSignal:
    """One entry discovered by :meth:`DeviceSession.browse` — a signal the device *offers*, whether
    or not it is configured. Backs the ``sb/browse`` diagnostics surface."""

    #: The stable id a consumer would configure or read.
    id: str
    #: A human label, when the device provides one.
    name: Optional[str]
    #: The device-native type, kept verbatim for diagnosis (``"REAL"``, ``"holding/uint16"``, …).
    type_name: str


@dataclass
class BrowsePage:
    """One page of a :meth:`DeviceSession.browse` enumeration. Browsing is **paged** because a
    device's address space can be large; ``next_cursor`` is set while more pages remain."""

    entries: List[BrowsedSignal]
    #: Opaque continuation token; pass it back as the next ``cursor``. ``None`` on the last page.
    next_cursor: Optional[str] = None


# =================================================================================================
# Seam errors — the vocabulary the command surface maps to the standardized error codes
# =================================================================================================

class DeviceError(Exception):
    """Why talking to the device failed. ``transient`` says whether reconnecting could help: a
    transient failure (link down, device busy) is worth a retry; a permanent one (bad endpoint,
    rejected credential, an address that does not exist) fails identically forever, so the
    supervisor backs off hard rather than hammering."""

    def __init__(self, message: str, transient: bool = True):
        super().__init__(message)
        self.transient = transient


class BrowseUnsupported(Exception):
    """The protocol has no discovery service. The default seam impl raises this, so an adapter that
    cannot browse stays honest (the command maps it to ``BROWSE_UNSUPPORTED``)."""


class BrowseFailed(Exception):
    """A mid-browse failure (a link error, a malformed reply). Maps to ``BROWSE_FAILED``."""


# The control-channel errors below are raised by the *control* seam (``adapter.WorkerControl``)
# that the command surface routes on — never by the raw ``DeviceSession``. They are the Python
# analog of the Rust control channel's reply variants: the command layer never touches the session
# directly, so every session-touching verb is confirmed through one of these.

class DeviceUnavailable(Exception):
    """The device task/session is not available (down, or shutting down). Maps to
    ``DEVICE_UNAVAILABLE``."""


class ReadFailed(Exception):
    """An on-demand read failed at the link. Maps to ``READ_FAILED``."""


class WriteRejected(Exception):
    """The device rejected one write (a per-entry failure, not a fatal one). Recorded as that
    entry's ``error`` in the batch result."""


class ReconnectFailed(Exception):
    """A ``reconnect`` attempt failed. Maps to ``RECONNECT_FAILED``."""


class RepollRefused(Exception):
    """A ``repoll`` was refused (the instance is paused). Maps to ``BAD_ARGS``."""


# =================================================================================================
# The seam itself
# =================================================================================================

class DeviceSession(ABC):
    """A live connection to one device. **This is the class you implement.**"""

    @abstractmethod
    def read_signals(self) -> List[Reading]:
        """Read the configured signals once.

        A read that fails for *one* signal should return that signal with :attr:`Quality.BAD`
        rather than failing the whole call — one dead register must not blind you to the other
        ninety-nine. Raise :class:`DeviceError` only when the *connection* is broken.
        """

    def read_named(self, ids: List[str]) -> List[Reading]:
        """Read a named subset **now** (backs ``sb/read``). The default reads everything and
        filters, which is correct for any backend; override it when your protocol can read a subset
        more cheaply. Raises :class:`DeviceError` only when the *connection* is broken."""
        wanted = set(ids)
        return [r for r in self.read_signals() if r.signal_id in wanted]

    @abstractmethod
    def write_signal(self, signal_id: str, value: Any) -> None:
        """Write a value back to the device. Raise :class:`DeviceError` if the write is rejected or
        the link is down."""

    def browse(self, cursor: Optional[str], max_entries: int) -> BrowsePage:
        """Enumerate the device's address space, one page at a time (backs ``sb/browse``).

        The default raises :class:`BrowseUnsupported` — a protocol with no discovery (Modbus, a
        fixed register map) is honest to leave it unimplemented. Override it when your protocol can
        enumerate (OPC UA browse, an EtherNet/IP tag list). Raise :class:`BrowseFailed` on a
        mid-browse link/protocol error.
        """
        raise BrowseUnsupported()

    def close(self) -> None:
        """Close the connection. Must be safe to call twice."""


class DeviceBackend(ABC):
    """Opens sessions. One factory per protocol."""

    @abstractmethod
    def kind(self) -> str:
        """The protocol's name, as it appears in config and in the published ``device.adapter``
        field."""

    def inventory(self, connection: Dict[str, Any]) -> List[SignalInfo]:
        """The signal inventory this backend exposes for a device, **without connecting** — read
        from config in a real adapter. Backs ``sb/signals`` (a config view, no device round-trip).
        The default is empty; the simulator returns a fixed pair so the command has something to
        show."""
        return []

    @abstractmethod
    def connect(self, connection: Dict[str, Any]) -> DeviceSession:
        """Connect to one device. Raise :class:`DeviceError` (transient when unreachable, permanent
        when the configuration is wrong)."""


# =================================================================================================
# The simulated backend
# =================================================================================================
#
# A real adapter replaces this with its protocol. It ships so that `python main.py` works with no
# hardware, and so the tests have something to talk to — and a backend you can run on a laptop is
# worth more than one you can only run next to a PLC.

#: The signals the simulator exposes — the ids it reads and the one it fails. A real backend derives
#: this from config; the simulator hard-codes it so ``sb/signals`` and ``sb/browse`` have content.
SIM_SIGNALS = [
    ("temperature-1", "Ambient temperature", "REAL"),
    ("pressure-1", "Line pressure", "REAL"),
]


class SimBackend(DeviceBackend):
    def kind(self) -> str:
        return "sim"

    def inventory(self, connection: Dict[str, Any]) -> List[SignalInfo]:
        return [SignalInfo(id=sid, name=name) for (sid, name, _ty) in SIM_SIGNALS]

    def connect(self, connection: Dict[str, Any]) -> DeviceSession:
        if not (connection or {}).get("endpoint"):
            # A missing endpoint will never fix itself: permanent, so the supervisor does not spend
            # the next hour reconnecting to nothing.
            raise DeviceError("no endpoint configured", transient=False)
        return SimSession()


class SimSession(DeviceSession):
    def __init__(self):
        self._tick = 0

    def read_signals(self) -> List[Reading]:
        self._tick += 1
        value = 20.0 + 5.0 * math.sin(self._tick / 10.0)
        return [
            Reading(
                signal_id="temperature-1",
                name="Ambient temperature",
                value=value,
                quality=Quality.GOOD,
                quality_raw="OK",
            ),
            # A signal the simulated device cannot currently read. It is published as BAD rather
            # than omitted, because "I could not read this" is information and silence is not.
            Reading(
                signal_id="pressure-1",
                name="Line pressure",
                value=None,
                quality=Quality.BAD,
                quality_raw="SENSOR_FAULT",
            ),
        ]

    def write_signal(self, signal_id: str, value: Any) -> None:
        # The sim accepts every write; a real backend encodes + sends it and raises DeviceError on
        # rejection.
        return None

    def browse(self, cursor: Optional[str], max_entries: int) -> BrowsePage:
        # A cursor means "the page after the last one" — the sim has nothing more. A real backend
        # pages a large address space and returns a next_cursor.
        if cursor:
            return BrowsePage(entries=[], next_cursor=None)
        entries = [
            BrowsedSignal(id=sid, name=name, type_name=ty) for (sid, name, ty) in SIM_SIGNALS
        ]
        return BrowsePage(entries=entries, next_cursor=None)


def make_backend(adapter: str) -> Optional[DeviceBackend]:
    """Instantiate the backend for a device's ``adapter``. A real adapter matches its protocol(s)
    here (``"modbus"`` -> ``ModbusBackend()``, …)."""
    if adapter == "sim":
        return SimBackend()
    return None
