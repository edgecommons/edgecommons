"""Configuration — numeric canonicalization (D-NC1 / D-NC2).

One shared rule for "this JSON number encodes an integer", applied as a pure pass over the
raw configuration document at the intake boundary and reused by every integer-typed field in
the library's own config sections.

Overview
--------
Configuration stores do not agree on how they encode a JSON number. A file or a ConfigMap
carries the literal ``5000``; the Greengrass configuration store round-trips every number
through a Java ``double`` and hands back ``5000.0``. Python does not crash on that difference
the way a statically typed reader does — it silently carries the float onwards, so the *same*
logical configuration becomes a different document depending on where it came from:
``isinstance(value, int)`` in component code fails, the effective configuration published on
the UNS ``cfg`` class differs per platform, and the envelope ``tags`` are typed differently on
the wire.

:func:`canonicalize_json_numbers` removes that difference once, at the boundary
(:class:`~edgecommons.config.manager.config_manager.ConfigManager` intake): every JSON number
whose value is an integer is rewritten as a Python ``int``, so everything downstream — schema
validation, component-supplied candidate validators, the committed snapshot,
``get_full_config()`` / ``get_global_config()`` / ``get_instance_config(..)``, the envelope
``tags``, and the published effective configuration — sees one canonical document whatever the
store did to it.

Semantics
---------
A number is rewritten as an integer **iff** it is a finite ``float`` with no fractional part
whose value lies inside the shared 64-bit window ``[-2^63, 2^64 - 1]``. Anything else is left
byte-identical: a fractional value, a value outside the window, ``NaN``, an infinity, and every
string, boolean, and ``None``. There is deliberately **no string or boolean coercion** (D-NC4) —
coercing ``"1.0"`` would corrupt legitimately-string settings, and no store this library targets
stringifies numbers. ``bool`` is excluded explicitly because it subclasses ``int``; converting
floats only is what avoids that trap.

Python integers are unbounded, so the window exists purely for parity of the rule with the
other three languages: the same document canonicalizes identically in Java, Python, and Rust.

The pass is pure and **idempotent** — a canonical document is a fixed point, so applying it at
both the pipeline intake and the snapshot boundary is harmless defense in depth. Object keys
are never touched, and the caller's document is never mutated.

Strict reads (D-NC2)
--------------------
Canonicalization widens what parses; it never rewrites a value the author did not write. The
readers here — :func:`require_non_negative_int`, :func:`require_non_negative_integral`,
:func:`as_integral_int` — are the single implementation every integer-typed field in the
library's own config sections uses. They accept an integer or an integral float and **reject** a
fractional value, a negative value in a non-negative setting, an out-of-range value, and a
non-number, loudly, with a message naming the offending value. Silently truncating or clamping a
configured value is a worse defect than refusing to start.

The messages mirror the canonical Java implementation
(``libs/java/.../config/ConfigNumbers.java``) verbatim; the exception type is ``ValueError``,
Python's counterpart to Java's ``IllegalArgumentException``.

Usage
-----
::

    from edgecommons.config import canonicalize_json_numbers

    document = {"component": {"instances": [{"pollIntervalMs": 5000.0}]}}
    canonical = canonicalize_json_numbers(document)
    assert canonical["component"]["instances"][0]["pollIntervalMs"] == 5000
    assert isinstance(canonical["component"]["instances"][0]["pollIntervalMs"], int)

Design choices
--------------
Read-side and unconditional: nothing rewrites a configuration store. A store already holding
doubles (the Greengrass store is cumulative — redeploying the original recipe does not clear it)
is read correctly on the very next start or configuration update, with no reset.
"""

import copy
import json
import math
from typing import Any, Optional

#: Lower bound of the canonicalization window: ``-2^63`` (the signed 64-bit minimum).
MIN_SIGNED_64 = -(2 ** 63)
#: Largest value representable as a signed 64-bit integer: ``2^63 - 1``.
MAX_SIGNED_64 = 2 ** 63 - 1
#: Upper bound of the canonicalization window: ``2^64 - 1`` (the unsigned 64-bit maximum).
MAX_UNSIGNED_64 = 2 ** 64 - 1
#: Largest value representable as a signed 32-bit integer: ``2^31 - 1`` (Java's ``int``).
MAX_SIGNED_32 = 2 ** 31 - 1

#: Uniform prefix for every numeric-configuration rejection (mirrors Java's ``ConfigNumbers``).
_ERROR_PREFIX = "configuration value "


def canonicalize_json_numbers(document: Any) -> Any:
    """Return a canonicalized deep copy of ``document``; the argument is never mutated.

    Walks objects and arrays recursively and rewrites every number whose value is integral and
    inside the shared 64-bit window as a Python ``int``. Keys, strings, booleans, ``None``,
    fractional numbers, and numbers outside the window are returned unchanged. The pass is pure
    and idempotent.

    Component code that obtains raw configuration JSON outside the config manager can apply the
    identical rule with this function; everything reached through the manager is canonical
    already.

    :param document: any JSON-shaped value (dict, list, scalar); ``None`` returns ``None``
    :returns: a canonical deep copy of ``document``
    """
    return _canonicalize(copy.deepcopy(document))


def require_non_negative_int(value: Any, path: str) -> int:
    """Read a configuration number as a non-negative ``int``, rejecting anything the author did
    not actually write (D-NC2).

    Accepts an integral value in any numeric encoding — ``5``, ``5.0``, and ``5e0`` are the same
    value and all parse. Raises :class:`ValueError` naming ``path`` for a non-number (``None``,
    string, boolean, object, array), ``NaN``/infinity, a fractional value (it is **never**
    truncated), a negative value (it is **never** clamped to zero or to a default), and a value
    larger than ``2^31 - 1``.

    :param value: the configured value
    :param path: the dotted configuration path used in the error message, e.g.
        ``"heartbeat.intervalSecs"``
    :returns: the value as a non-negative ``int``
    :raises ValueError: if the value is not a non-negative whole number in range
    """
    result = require_non_negative_integral(value, path)
    if result > MAX_SIGNED_32:
        raise ValueError(
            f"{_ERROR_PREFIX}{_quote(path)} is out of range for a 32-bit integer: {result}"
        )
    return result


def require_non_negative_integral(value: Any, path: str) -> int:
    """The shared non-negative integral read: validate that ``value`` is a finite, whole,
    non-negative number inside the unsigned 64-bit window and return it as an ``int``.

    Same acceptance and rejection rules as :func:`require_non_negative_int` without the 32-bit
    bound — the reader for settings whose canonical type is a 64-bit unsigned integer
    (``credentials.vault.keepVersions``, ``parameters.refreshIntervalSecs``,
    ``logging.publish.maxRecordBytes``, …).

    :param value: the configured value
    :param path: the dotted configuration path used in the error message
    :returns: the value as a non-negative ``int``
    :raises ValueError: if the value is not a non-negative whole number in range
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(
            f"{_ERROR_PREFIX}{_quote(path)} must be a number, but was {_describe(value)}"
        )
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(
                f"{_ERROR_PREFIX}{_quote(path)} must be a finite number,"
                f" but was {_render(value)}"
            )
        if not value.is_integer():
            raise ValueError(
                f"{_ERROR_PREFIX}{_quote(path)} must be a whole number,"
                f" but was {_render(value)}"
            )
    if value < 0:
        raise ValueError(
            f"{_ERROR_PREFIX}{_quote(path)} must not be negative, but was {_render(value)}"
        )
    result = int(value)
    if result > MAX_UNSIGNED_64:
        raise ValueError(
            f"{_ERROR_PREFIX}{_quote(path)} is out of range for a 64-bit integer: {result}"
        )
    return result


def as_integral_int(value: Any) -> Optional[int]:
    """Read ``value`` as a non-negative ``int`` for a probe that has no error channel: ``None``
    for a fractional, negative, out-of-range, or non-numeric value.

    Used by the free-form accessors whose documented contract is to fall back to their schema
    default rather than to fail. Sharing the rule means they never truncate ``5.5`` to ``5`` or
    clamp ``-5.0`` to ``0`` on the way there.

    :param value: the configured value
    :returns: the value as a non-negative ``int``, or ``None`` when it is not one
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if isinstance(value, float):
        integral = _integral_value(value)
        if integral is None:
            return None
        value = integral
    return value if 0 <= value <= MAX_UNSIGNED_64 else None


def _canonicalize(value: Any) -> Any:
    """Rewrite ``value`` in place (containers) and return its canonical form.

    Containers are rewritten and returned as-is; a scalar has no container to write into, so the
    canonical replacement is returned instead. ``value`` is already a private copy when this is
    reached through :func:`canonicalize_json_numbers`.
    """
    if isinstance(value, dict):
        # Keys are never touched — only the mapped values are rewritten.
        for key, entry in list(value.items()):
            value[key] = _canonicalize(entry)
        return value
    if isinstance(value, list):
        for index, entry in enumerate(value):
            value[index] = _canonicalize(entry)
        return value
    return _canonical_number(value)


def _canonical_number(value: Any) -> Any:
    """The integer form of ``value``, or ``value`` unchanged.

    ``bool`` and ``int`` are already canonical (and ``bool`` must never be treated as a number
    here — it subclasses ``int``); strings, ``None``, and every other type are never coerced
    (D-NC4). Only a ``float`` can move, and only when it encodes an integer.
    """
    if not isinstance(value, float):
        return value
    integral = _integral_value(value)
    return value if integral is None else integral


def _integral_value(value: float) -> Optional[int]:
    """The exact integer value of ``value`` when it is finite, whole, and inside the shared
    64-bit window ``[-2^63, 2^64 - 1]``; otherwise ``None``.

    The comparison against the window is exact: Python compares an ``int`` with a ``float`` by
    value, without converting either, so ``2.0 ** 64`` is correctly recognized as *outside* the
    unsigned window rather than rounding onto its upper bound.
    """
    if not math.isfinite(value) or not value.is_integer():
        return None
    if not MIN_SIGNED_64 <= value <= MAX_UNSIGNED_64:
        return None
    return int(value)


def _quote(path: str) -> str:
    return f"'{path}'"


def _describe(value: Any) -> str:
    """Render a rejected value for an error message without ever raising."""
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, dict):
        return "an object"
    if isinstance(value, (list, tuple)):
        return "an array"
    if isinstance(value, str):
        return json.dumps(value)   # quoted, exactly as it appears in the document
    return repr(value)


def _render(value: Any) -> str:
    """Render a rejected *number* as it was written (``5.5``, ``-5.0``, ``nan``)."""
    return repr(value) if isinstance(value, float) else str(value)
