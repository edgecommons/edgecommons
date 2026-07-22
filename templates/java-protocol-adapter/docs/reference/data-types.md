# Reference — Data Types

> This documents the generated scaffold; rewrite it as you build the component out.

How device values map to EdgeCommons message values, in both directions. The scaffold's simulated
backend only produces numbers and one null-with-`BAD`-quality reading, but the seam (`Device.Reading`,
`Device.Quality`) is general — this page describes the mapping every backend should honor, and what
the sim currently exercises.

## The `Reading` shape

Every value the seam hands upward is a `Device.Reading(signalId, name, value, quality, qualityRaw)`.
`value` is a `com.google.gson.JsonElement` — whatever JSON type your protocol value naturally maps
to; the sim uses `JsonPrimitive` (numbers) and `JsonNull`.

| EdgeCommons JSON type | When to use it |
|---|---|
| number (`JsonPrimitive`) | Numeric registers/tags — the common case. Map your protocol's integer/float types here. |
| boolean (`JsonPrimitive`) | Discrete/coil-style values. |
| string (`JsonPrimitive`) | Text, enums, or a native type with no better JSON representation (a timestamp, an identifier). |
| array (`JsonArray`) | An array-valued signal; encode each element by its own scalar rule. |
| null (`JsonNull`) | No value could be read this cycle — pair it with `Quality.BAD` or `Quality.UNCERTAIN`, never `GOOD`. |

## Quality

| `Device.Quality` | Meaning | Sim usage |
|---|---|---|
| `GOOD` | The value is trustworthy. | `temperature-1` every poll. |
| `BAD` | The value could not be obtained, or is known wrong. | `pressure-1` every poll (`qualityRaw: "SENSOR_FAULT"`) — deliberately, to show a failed read is reported, not dropped. |
| `UNCERTAIN` | Obtained, but with reduced confidence (stale cache, out-of-range, a device warning alongside the value). | Not produced by the sim; a real backend should use it for values a real protocol marks suspect rather than definitively bad. |

`qualityRaw` always carries the protocol's own native status string, for diagnostics that need more
than the three normalized buckets.

## What the simulator exercises today

| Signal | Type | Behavior |
|---|---|---|
| `temperature-1` | number | A sine wave, `GOOD`, every poll. |
| `pressure-1` | null | Always `BAD` / `SENSOR_FAULT` — a worked example of reporting a failed read. |

Extend this table as `Device.java` starts talking to a real protocol — document every native type it
can produce and how each maps to the JSON value / quality above, the way the reference adapters
(`opcua-adapter/docs/reference/data-types.md`, `modbus-adapter`) document theirs.
