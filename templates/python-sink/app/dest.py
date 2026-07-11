"""The destination: what a *sink* delivers to.

A sink consumes work and hands it to somewhere outside EdgeCommons -- a filesystem, an object store,
an HTTP endpoint, a database. :class:`Destination` is the seam. Implement it once per backend;
everything above it (retry, verification, reporting) is written against the abstraction and never
learns what a bucket is.

The contract, and why each clause is there
------------------------------------------
* **`deliver` is the commit.** When it returns, the item is live at its final, *stable* key. Not
  staged, not pending -- live.
* **The key is deterministic.** The same item always lands at the same place, so a redelivery is an
  **idempotent overwrite** rather than a duplicate. This is what makes retry safe: a sink that cannot
  retry without duplicating cannot retry at all.
* **`verify` runs before the source is released.** The whole point of a sink is that it is the last
  thing standing between data and its destination. Releasing the source because `deliver` returned --
  without checking that what landed is what you sent -- is how you lose the only copy.

**This module deliberately does not import ``edgecommons``.** The destination, the error taxonomy and
the retry policy are pure logic, so they are unit-testable with no broker and no transport. The
library-facing wiring lives in ``app/<<COMPONENTNAME>>.py``.
"""
import os
import random
from abc import ABC, abstractmethod
from typing import Any, Dict, Optional


class Item:
    """One unit of work to deliver: an opaque payload plus the stable key it belongs at."""

    __slots__ = ("key", "data")

    def __init__(self, key: str, data: bytes):
        #: The stable, deterministic key. Redelivering the same item overwrites in place.
        self.key = key
        self.data = data

    def __repr__(self) -> str:  # pragma: no cover - diagnostics only
        return f"Item(key={self.key!r}, {len(self.data)} bytes)"


class Delivered:
    """Proof of what landed, returned by :meth:`Destination.deliver` and checked by
    :meth:`Destination.verify`."""

    __slots__ = ("bytes_written",)

    def __init__(self, bytes_written: int):
        self.bytes_written = bytes_written

    def __eq__(self, other) -> bool:
        return isinstance(other, Delivered) and other.bytes_written == self.bytes_written

    def __repr__(self) -> str:  # pragma: no cover - diagnostics only
        return f"Delivered(bytes_written={self.bytes_written})"


class DeliverError(Exception):
    """Why a delivery failed -- and, crucially, **whether retrying could ever help**.

    Getting this wrong is expensive in both directions: retrying a permanent failure burns the budget
    and floods the log; giving up on a transient one loses data that a second attempt would have
    delivered.
    """

    def __init__(self, message: str, transient: bool):
        super().__init__(message)
        self.transient = transient

    @staticmethod
    def transient_failure(message: str) -> "DeliverError":
        """The world may differ next time: a timeout, a 503, a full disk someone will empty."""
        return DeliverError(f"transient: {message}", transient=True)

    @staticmethod
    def permanent_failure(message: str) -> "DeliverError":
        """It will fail identically forever: bad credentials, a malformed key, a missing bucket."""
        return DeliverError(f"permanent: {message}", transient=False)


class Destination(ABC):
    """A place a sink delivers to. **This is the class you implement.**"""

    @abstractmethod
    def kind(self) -> str:
        """Its kind, as named in config (`local`, `s3`, ...)."""

    @abstractmethod
    def deliver(self, item: Item) -> Delivered:
        """Deliver the item to its stable key. Returning means it is **live**, not staged.

        :raises DeliverError: classified transient (retry may help) or permanent (it never will)
        """

    @abstractmethod
    def verify(self, item: Item, delivered: Delivered) -> None:
        """Confirm that what landed is what was sent -- **before** the source is released.

        :raises DeliverError: when what landed is not what was sent
        """


class LocalDestination(Destination):
    """A local-filesystem destination.

    Small, but it demonstrates the two things every destination must get right: **write to a temp file
    and rename** (``os.replace`` is atomic, so a reader never observes a half-written object, and a
    crash mid-write leaves no corrupt artifact at the real key), and **land at a deterministic key**
    so a redelivery overwrites rather than duplicates.
    """

    def __init__(self, root: str):
        self.root = root

    def kind(self) -> str:
        return "local"

    def deliver(self, item: Item) -> Delivered:
        final_path = os.path.join(self.root, *item.key.split("/"))
        parent = os.path.dirname(final_path) or self.root

        try:
            os.makedirs(parent, exist_ok=True)
        except OSError as e:
            # A directory we cannot create is usually a permission or a path problem, and those do
            # not fix themselves -- but a full disk does. Transient is the safer default: a wrongly
            # transient failure wastes retries, a wrongly permanent one loses data.
            raise DeliverError.transient_failure(f"creating the destination directory: {e}") from e

        tmp = os.path.join(parent, f".{_sanitize(item.key)}.partial")
        try:
            with open(tmp, "wb") as f:
                f.write(item.data)
                f.flush()
                os.fsync(f.fileno())  # the bytes are on the disk before anything points at them
            # The atomic step. Until this returns, nothing exists at the real key.
            os.replace(tmp, final_path)
        except OSError as e:
            _unlink_quietly(tmp)
            raise DeliverError.transient_failure(f"writing {item.key}: {e}") from e

        return Delivered(len(item.data))

    def verify(self, item: Item, delivered: Delivered) -> None:
        path = os.path.join(self.root, *item.key.split("/"))
        try:
            landed = os.path.getsize(path)
        except OSError as e:
            raise DeliverError.transient_failure(f"stat-ing the delivered object: {e}") from e

        if landed != delivered.bytes_written:
            # The object is there but wrong. Do NOT release the source.
            raise DeliverError.transient_failure(
                f"size mismatch: wrote {delivered.bytes_written} bytes, found {landed}"
            )


def build_destination(cfg: Dict[str, Any]) -> Destination:
    """Build a destination from its config object. Add a branch as you add a backend -- and the
    matching variant in `config.schema.json`'s `destination` definition.

    :raises ValueError: on an unknown or malformed destination
    """
    if not isinstance(cfg, dict):
        raise ValueError(f"`destination` must be an object, got: {cfg!r}")
    kind = cfg.get("type")
    if kind == "local":
        unknown = set(cfg) - {"type", "path"}
        if unknown:
            raise ValueError(f"unknown destination key(s): {sorted(unknown)}")
        path = cfg.get("path")
        if not isinstance(path, str) or not path:
            raise ValueError("a `local` destination requires a non-empty `path`")
        return LocalDestination(path)
    raise ValueError(f"unknown destination type: {kind!r}")


def _sanitize(key: str) -> str:
    """Keep a temp-file name from escaping its directory."""
    return key.replace("/", "_").replace("\\", "_")


def _unlink_quietly(path: str) -> None:
    try:
        os.unlink(path)
    except OSError:
        pass


# --- retry -------------------------------------------------------------------------------------

DEFAULT_BASE_DELAY_MS = 1_000
DEFAULT_MAX_DELAY_MS = 900_000      # 15 min
DEFAULT_GIVE_UP_AFTER_MS = 3_600_000  # 1 hour
#: The exponent is clamped, so a long outage cannot grow 2**attempt into nonsense.
_MAX_EXPONENT = 20


class RetryPolicy:
    """How hard, and for how long, to keep trying.

    Note the give-up is a **time budget**, not an attempt count. "Twenty attempts" means something
    different at 1 s and at 15 min of backoff; "keep trying for an hour" means the same thing at every
    cadence, and it is what an operator can actually reason about.
    """

    __slots__ = ("base_delay_ms", "max_delay_ms", "give_up_after_ms")

    def __init__(self, base_delay_ms: int = DEFAULT_BASE_DELAY_MS,
                 max_delay_ms: int = DEFAULT_MAX_DELAY_MS,
                 give_up_after_ms: int = DEFAULT_GIVE_UP_AFTER_MS):
        self.base_delay_ms = base_delay_ms
        self.max_delay_ms = max_delay_ms
        self.give_up_after_ms = give_up_after_ms

    def delay_ms(self, attempt: int, rand01: Optional[float] = None) -> int:
        """Full-jitter exponential backoff: a random delay in ``[0, min(cap, base * 2**attempt))``.

        The jitter is not decoration. Without it, every component that lost the same endpoint retries
        at the same instant, and the endpoint -- which is probably struggling already -- is hit by a
        synchronized thundering herd on every backoff boundary.
        """
        if rand01 is None:
            rand01 = random.random()
        exponent = min(max(attempt, 0), _MAX_EXPONENT)
        window = min(self.base_delay_ms * (2 ** exponent), self.max_delay_ms)
        return int(min(max(rand01, 0.0), 1.0) * window)

    def budget_spent(self, elapsed_ms: float) -> bool:
        """Has the time budget run out? Then the item is *exhausted* -- data that did not arrive."""
        return elapsed_ms >= self.give_up_after_ms


_RETRY_KEYS = {"baseDelayMs", "maxDelayMs", "giveUpAfterMs"}


def parse_retry(cfg: Optional[Dict[str, Any]], defaults: Optional[Dict[str, Any]] = None) -> RetryPolicy:
    """Parse a `retry` object, applying `component.global.defaults.retry` for anything it omits.

    :raises ValueError: on an unknown key or a non-positive delay
    """
    merged: Dict[str, Any] = dict(defaults or {})
    if cfg is not None:
        if not isinstance(cfg, dict):
            raise ValueError(f"`retry` must be an object, got: {cfg!r}")
        merged.update(cfg)

    unknown = set(merged) - _RETRY_KEYS
    if unknown:
        raise ValueError(f"unknown retry key(s): {sorted(unknown)}")

    return RetryPolicy(
        base_delay_ms=_positive_int(merged, "baseDelayMs", DEFAULT_BASE_DELAY_MS),
        max_delay_ms=_positive_int(merged, "maxDelayMs", DEFAULT_MAX_DELAY_MS),
        give_up_after_ms=_positive_int(merged, "giveUpAfterMs", DEFAULT_GIVE_UP_AFTER_MS),
    )


# --- sink configuration ------------------------------------------------------------------------

#: Bounded, like every queue that faces a network.
DEFAULT_MAX_QUEUE = 256

_SINK_KEYS = {"id", "subscribe", "destination", "retry", "maxQueue"}


class SinkConfig:
    """One sink == one entry of ``component.instances[]``."""

    __slots__ = ("id", "subscribe", "destination", "retry", "max_queue")

    def __init__(self, id: str, subscribe: str, destination: Dict[str, Any],
                 retry: RetryPolicy, max_queue: int):
        self.id = id
        self.subscribe = subscribe
        self.destination = destination
        self.retry = retry
        self.max_queue = max_queue

    def build_destination(self) -> Destination:
        return build_destination(self.destination)


def parse_sink(inst: Dict[str, Any], defaults: Optional[Dict[str, Any]] = None) -> SinkConfig:
    """Parse one ``component.instances[]`` entry, applying ``component.global.defaults``.

    Unknown keys are **rejected, not ignored** -- a config knob that silently does nothing is the
    worst kind of bug to find in the field.

    :raises ValueError: on a missing/ill-typed/unknown key, or a destination that does not build
    """
    defaults = defaults or {}
    if not isinstance(inst, dict):
        raise ValueError(f"an instance must be an object, got: {inst!r}")

    unknown = set(inst) - _SINK_KEYS
    if unknown:
        raise ValueError(f"unknown sink key(s): {sorted(unknown)}")

    sink_id = _require_str(inst, "id")
    subscribe = _require_str(inst, "subscribe")

    destination = inst.get("destination")
    build_destination(destination)  # fail here, at config time -- not on the first message

    retry = parse_retry(inst.get("retry"), defaults.get("retry"))
    max_queue = _positive_int(inst, "maxQueue", defaults.get("maxQueue", DEFAULT_MAX_QUEUE))

    return SinkConfig(sink_id, subscribe, destination, retry, max_queue)


def key_for(sink_id: str, topic: str, uuid: str) -> str:
    """A stable, deterministic key for a message.

    Deterministic is the whole point: the same message must always resolve to the same key, or a retry
    duplicates instead of overwriting.
    """
    leaf = topic.rsplit("/", 1)[-1] or "message"
    return f"{sink_id}/{leaf}/{uuid}.json"


def _require_str(cfg: Dict[str, Any], key: str) -> str:
    value = cfg.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"`{key}` is required and must be a non-empty string")
    return value


def _positive_int(cfg: Dict[str, Any], key: str, fallback: int) -> int:
    value = cfg.get(key, fallback)
    if isinstance(value, bool) or not isinstance(value, int) or value < 1:
        raise ValueError(f"`{key}` must be a positive integer, got {value!r}")
    return value
