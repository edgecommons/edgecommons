"""
Durable store-and-forward drain for the direct CloudWatch metric target.

Reuses the ``ggstreamlog`` durable log + at-least-once export engine via a **host-callback sink**
(``StreamService.open_with_callback``). Each metric datum is serialized to a compact JSON record
``{"namespace": ..., "datum": {...}}`` (partition key = namespace) and appended to a single
``callback`` stream; the native export engine drives :meth:`CloudWatchDrain.drain_batch` on its
background thread, which:

* deserializes the batch,
* groups datums by namespace (``PutMetricData`` takes one namespace per call),
* drops datums whose timestamp is outside CloudWatch's accept window (~2 weeks past / ~2 hours
  future) — retry-forever cannot fix an aged-out timestamp — counting them in ``dropped_stale``,
* chunks to <=1000 datums and <=~1 MB per request,
* calls boto3 ``put_metric_data``, and maps the result onto the engine's
  ``AllAcked | Partial | Failed`` outcome so the buffer checkpoint only advances for datums that
  were sent (or deliberately dropped as stale).

This gives the CloudWatch target flat memory + a disk-bounded backlog across lengthy disconnects:
datums accumulate on disk (``onFull: dropOldest`` past ``maxDiskBytes``), drain on reconnect, and
stale datums are dropped + counted rather than wedging the stream forever.

The legacy in-memory batching path stays in ``cloudwatch.py`` for ``buffer.type == memory`` (or no
buffer section).
"""
from __future__ import annotations

import json
import logging
import threading
import time
from typing import Dict, List, Optional, Tuple

# CloudWatch accepts timestamps roughly two weeks in the past and two hours in the future.
# Datums outside this window are rejected by the API forever, so we drop them on drain.
_STALE_PAST_SECS = 14 * 24 * 3600
_STALE_FUTURE_SECS = 2 * 3600

# PutMetricData hard limits.
_MAX_DATUMS_PER_REQUEST = 1000
# ~1 MB request cap; stay conservatively under it (headroom for the request envelope).
_MAX_REQUEST_BYTES = 900 * 1024

logger = logging.getLogger("CloudWatchDrain")


def serialize_datum(namespace: str, datum: dict) -> bytes:
    """Serialize one ``(namespace, datum)`` to the compact JSON record stored in the buffer.

    The ``Timestamp`` (a ``datetime`` or epoch ``float``/``int`` from the CloudWatch datum builder)
    is normalized to epoch seconds so the drain can apply the stale-drop window without importing
    timezone state.
    """
    out = dict(datum)
    ts = out.get("Timestamp")
    if isinstance(ts, (int, float)):
        out["Timestamp"] = float(ts)
    elif hasattr(ts, "timestamp"):  # datetime
        out["Timestamp"] = float(ts.timestamp())
    elif ts is None:
        out["Timestamp"] = time.time()
    return json.dumps({"namespace": namespace, "datum": out}, separators=(",", ":")).encode("utf-8")


def deserialize_record(payload: bytes) -> Tuple[str, dict]:
    """Inverse of :func:`serialize_datum`: ``payload bytes -> (namespace, datum)``."""
    doc = json.loads(payload.decode("utf-8"))
    return doc["namespace"], doc["datum"]


def _is_stale(datum: dict, now: float) -> bool:
    ts = datum.get("Timestamp")
    if not isinstance(ts, (int, float)):
        return False
    return ts < now - _STALE_PAST_SECS or ts > now + _STALE_FUTURE_SECS


def _datum_to_api(datum: dict) -> dict:
    """Convert a stored datum (epoch-seconds Timestamp) to the PutMetricData wire shape."""
    out = dict(datum)
    ts = out.get("Timestamp")
    if isinstance(ts, (int, float)):
        # boto3 accepts an epoch float for Timestamp, but a datetime is the documented type;
        # keep the float (botocore serializes it) to avoid a tz dependency.
        out["Timestamp"] = float(ts)
    return out


def chunk_datums(datums: List[dict]) -> List[List[dict]]:
    """Split datums into request-sized chunks (<=1000 datums and <=~1 MB serialized each)."""
    chunks: List[List[dict]] = []
    current: List[dict] = []
    current_bytes = 0
    for d in datums:
        size = len(json.dumps(d, default=str).encode("utf-8"))
        too_many = len(current) >= _MAX_DATUMS_PER_REQUEST
        too_big = current and current_bytes + size > _MAX_REQUEST_BYTES
        if too_many or too_big:
            chunks.append(current)
            current = []
            current_bytes = 0
        current.append(d)
        current_bytes += size
    if current:
        chunks.append(current)
    return chunks


class CloudWatchDrain:
    """The host-callback sink for the durable CloudWatch buffer.

    Holds the boto3 client + the stale-drop counter; :meth:`drain_batch` is registered as the
    ``ggstreamlog`` callback. Thread-safe: ``drain_batch`` runs on the native export engine thread.
    """

    def __init__(self, cloudwatch_client):
        self._client = cloudwatch_client
        self._lock = threading.Lock()
        self.dropped_stale = 0
        self.last_error: Optional[str] = None

    def drain_batch(self, records):
        """Native export-engine callback. Returns a ``SinkOutcome`` (see ``StreamService``):

        * ``None`` if every record was accepted (or dropped as stale) — the batch commits.
        * a list of failed offsets if one or more namespaces could not be sent — those records are
          re-delivered on the next attempt (at-least-once); the rest commit.
        """
        now = time.time()
        # Group live datums by namespace, tracking which offset each datum came from so a failed
        # namespace can report exactly the offsets to retry.
        by_ns: Dict[str, List[Tuple[int, dict]]] = {}
        dropped_now = 0
        for offset, _pk, _ts_ms, payload in records:
            try:
                namespace, datum = deserialize_record(payload)
            except (ValueError, KeyError) as e:
                # A corrupt/undeserializable record can never be sent; drop it (commit) rather than
                # wedge the stream forever. Counted as stale-style loss.
                logger.warning("dropping undeserializable buffered metric at offset %d: %s", offset, e)
                dropped_now += 1
                continue
            if _is_stale(datum, now):
                dropped_now += 1
                continue
            by_ns.setdefault(namespace, []).append((offset, datum))

        if dropped_now:
            with self._lock:
                self.dropped_stale += dropped_now
            logger.info("dropped %d stale/undeliverable metric datum(s) on drain", dropped_now)

        failed_offsets: List[int] = []
        for namespace, items in by_ns.items():
            offsets = [o for o, _ in items]
            datums = [_datum_to_api(d) for _, d in items]
            if not self._send_namespace(namespace, datums):
                failed_offsets.extend(offsets)

        if failed_offsets:
            # Partial: only the offsets we could not send are retried; everything else (sent +
            # stale-dropped) is committed.
            return failed_offsets
        return None  # AllAcked

    def _send_namespace(self, namespace: str, datums: List[dict]) -> bool:
        """Send all datums for one namespace, chunked. Returns True iff every chunk was accepted."""
        for chunk in chunk_datums(datums):
            try:
                self._client.put_metric_data(Namespace=namespace, MetricData=chunk)
            except Exception as e:  # throttle / 5xx / transport / disconnect -> retry the batch
                with self._lock:
                    self.last_error = str(e)
                logger.warning(
                    "put_metric_data failed for namespace '%s' (%d datums); will retry: %s",
                    namespace, len(chunk), e,
                )
                return False
        return True

    def stats(self) -> dict:
        """Self-observability snapshot (drain-side); buffer depth comes from StreamService.stats."""
        with self._lock:
            return {"dropped_stale": self.dropped_stale, "last_error": self.last_error}
