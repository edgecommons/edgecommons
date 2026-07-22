# Reference — Data Types

*This documents the generated scaffold; rewrite it as you build the component out.*

How a reading becomes a published value, and the quality model every signal carries. This page
describes the seam (`<<SNAKENAME>>/device.py`), not any specific protocol's wire format — your
`DeviceSession` implementation decides what a "type" means for your device.

## The value/quality objects

| Type | Field | Meaning |
|---|---|---|
| `Reading` | `signal_id` | The canonical, stable id the rest of the fleet keys on (e.g. `ns=3;i=1001`, or a Modbus-style `u1/holding/0/float32`). |
| | `name` | A human label, when the backend has one. Optional. |
| | `value` | The decoded value — `None` when the read failed and there is nothing to report. |
| | `quality` | One of `Quality.GOOD` / `Quality.BAD` / `Quality.UNCERTAIN`. |
| | `quality_raw` | The protocol-native status code, kept verbatim for diagnosis. |
| `SignalInfo` | `id`, `name` | One entry of the `sb/signals` inventory — known from config/backend **without a device round-trip**. |
| `BrowsedSignal` | `id`, `name`, `type_name` | One entry discovered by `browse()` — a signal the device *offers*, whether or not it is configured. `type_name` is the device-native type, kept verbatim (e.g. `"REAL"`, `"holding/uint16"`). |
| `BrowsePage` | `entries`, `next_cursor` | One page of a `browse()` enumeration; `next_cursor` is set while more pages remain. |

## Quality — normalized, protocol-independent

| Token | Meaning |
|---|---|
| `GOOD` | The value is trustworthy. |
| `BAD` | The value could not be obtained, or is known wrong (e.g. a sensor fault). `value` is typically `None` alongside `BAD`. |
| `UNCERTAIN` | The backend answered but flags reduced confidence — a stale cached read, a value outside its calibrated range, a sensor that warned. Unused by the bundled simulator; expect real backends to use it constantly. |

The protocol's own status code always survives in `quality_raw`, verbatim, for diagnosis — the
normalized token is what a consumer gates logic on; the raw code is what an operator reads in a log
or a diagnostics panel.

## Why a failed read still publishes

A signal that silently stops updating is indistinguishable from one that simply isn't changing. So
`_publish_reading` (`adapter.py`) treats `value is None` as information, not an omission: it
publishes a `BAD` sample with the exception/fault text in `qualityRaw`, through the pre-built-body
path (the `data()` facade's `samples[]`/`add_sample` shape cannot express "no value at all"). The
bundled simulator demonstrates this on every run — `pressure-1` always publishes `BAD`/
`SENSOR_FAULT` alongside `temperature-1`'s healthy `GOOD` reading.

## The simulated backend's signals

| `signal_id` | `name` | Behavior |
|---|---|---|
| `temperature-1` | Ambient temperature | A sine-wave `GOOD` reading, always succeeds. |
| `pressure-1` | Line pressure | Always `BAD`, `qualityRaw: "SENSOR_FAULT"` — demonstrates the failure path deliberately. |

`SimSession.write_signal` accepts any write for any signal id; a real backend encodes and sends the
value, raising `DeviceError` on rejection (mapped to a per-entry `sb/write` failure, not a fatal one).

## What your protocol needs to define

When you implement `DeviceSession` for a real protocol, decide and document:

- **The stable `signal_id` scheme** — how a register/tag/node maps to a canonical id that survives
  config changes (address renumbering, a tag rename) and is what `writes.allow` and `sb/read`/
  `sb/write` signal-refs key on.
- **What counts as `BAD` vs `UNCERTAIN`** for your protocol's native status/quality codes.
- **Whether `browse()` is supported** — override it if your protocol has discovery; leave the default
  (`BrowseUnsupported`) if it doesn't, so `sb/browse` answers honestly with `BROWSE_UNSUPPORTED`
  rather than an empty success.
- **Value typing** — this scaffold treats a signal's decoded `value` as whatever Python type your
  protocol naturally produces (`bool`, `int`, `float`, `str`); JSON has no separate integer/float
  distinction, so a consumer whose JSON parser uses IEEE-754 doubles may lose precision on very large
  integers.
