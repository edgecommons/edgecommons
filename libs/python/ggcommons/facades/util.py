"""Internal helpers shared by the ``data()``/``events()``/``app()`` publish facades:
channel-token sanitization and the injected-clock ISO-8601 formatting/parsing the
``data()`` facade needs for ``serverTs`` defaults and the stream route's producer
timestamp. Library-internal -- not part of the public facade surface.
"""
import re
from datetime import datetime, timezone

from ggcommons.config.manager.config_manager import ConfigManager

_ISO_RE = re.compile(
    r"^(?P<base>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?P<frac>\.\d+)?(?P<tz>Z|[+-]\d{2}:?\d{2})?$"
)


def sanitize_channel_path(path: str) -> str:
    """Sanitizes a ``/``-separated channel path token-by-token (each token through
    :meth:`ConfigManager.sanitize`), mirroring the Java facades' ``channelToken``/
    ``AppFacade`` channel sanitization. A raw signal/app path with ``/`` becomes one UNS
    channel token per ``/``-separated segment (e.g. ``"press12/temperature"`` -> two
    channel tokens; ``"a+b"`` -> ``"a_b"``).
    """
    return "/".join(ConfigManager.sanitize(token) for token in path.split("/"))


def format_instant(dt: datetime) -> str:
    """Formats a timezone-aware ``datetime`` as an ISO-8601 UTC string ending in ``Z``,
    matching Java's ``Instant.toString()`` -- no fractional part when the sub-second
    component is zero (so a whole-second fixed clock produces exactly
    ``"2026-07-01T12:00:00Z"``, the form the ``uns-test-vectors`` goldens pin).
    """
    dt = dt.astimezone(timezone.utc)
    if dt.microsecond == 0:
        return dt.strftime("%Y-%m-%dT%H:%M:%S") + "Z"
    if dt.microsecond % 1000 == 0:
        return dt.strftime("%Y-%m-%dT%H:%M:%S") + f".{dt.microsecond // 1000:03d}Z"
    return dt.strftime("%Y-%m-%dT%H:%M:%S") + f".{dt.microsecond:06d}Z"


def parse_iso_to_epoch_millis(ts: str) -> int:
    """Best-effort ISO-8601 timestamp -> epoch millis, used for the ``data()`` stream
    route's producer timestamp (the stream record's ``timestamp_ms``). Tolerates an
    arbitrary fractional-second digit count (a caller-supplied ``sourceTs``/``serverTs``
    is not constrained to milli/micro precision). Falls back to the current time when the
    timestamp does not parse -- mirroring the Java implementation's fallback to
    ``System.currentTimeMillis()``.
    """
    try:
        match = _ISO_RE.match(ts)
        if not match:
            raise ValueError(f"unparseable timestamp: {ts!r}")
        base = match.group("base")
        frac = match.group("frac") or ""
        tz = match.group("tz") or "Z"
        if tz == "Z":
            tz = "+00:00"
        elif len(tz) == 5:  # "+HHMM" -> "+HH:MM"
            tz = tz[:3] + ":" + tz[3:]
        if frac:
            digits = frac[1:]
            micros = (digits + "000000")[:6]
            frac = "." + micros
        parsed = datetime.fromisoformat(base + frac + tz)
        return int(parsed.timestamp() * 1000)
    except Exception:  # noqa: BLE001 - deliberate fallback, mirrors the Java facade
        return int(datetime.now(timezone.utc).timestamp() * 1000)
