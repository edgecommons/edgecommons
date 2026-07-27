# Reference — Data Types

> This documents the generated scaffold; rewrite it as you build the component out.

How device values map to EdgeCommons message values, in both directions. The scaffold's simulated
backend only produces numbers and one null-with-`BAD`-quality reading, but the seam (`Device.Reading`,
`Device.Quality`) is general — this page describes the mapping every backend should honor, and what
the sim currently exercises.

## The `Reading` shape

Every value the seam hands upward is a
`Device.Reading(signalId, name, value, quality, qualityRaw, sourceTs, captureTs, receivedTs)`;
the 5-argument constructor leaves the three trailing timestamps null (the common direct-client
form). `value` is a `com.google.gson.JsonElement` — whatever JSON type your protocol value
naturally maps to; the sim uses `JsonPrimitive` (numbers) and `JsonNull`.

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

## Timestamps

A reading carries up to three optional ISO-8601 UTC timestamps — the seam's slice of the
four-slot model (`docs/SOUTHBOUND.md` §2). None is ever synthesized from another:

| Slot | Meaning | Who sets it |
|---|---|---|
| `sourceTs` | The **machine** timestamp: device/field-authored time. | The backend, only when the protocol supplied it. |
| `captureTs` | The **capture** timestamp: the moment the protocol read the value — a mediating server's stamp (OPC UA server, MTConnect agent). | The backend, only when a mediating server provides one. A direct-client protocol leaves it null: its receive moment IS the capture moment. |
| `receivedTs` | The **adapter receive** timestamp. | The worker (`Wiring.stampReceived`), at read completion, when the backend left it null. |

On publish (`Wiring.toSample` — the same mapping on the GOOD and BAD/null paths):

- `captureTs` becomes the sample's `serverTs`; when absent, `receivedTs` takes its place.
- `sourceTs` is passed through verbatim, only when present.
- `receivedTs` rides as a per-sample `receivedTs` extra field only when it differs from the
  effective `serverTs` — identical stamps would make the extra noise, so it is omitted.

The simulator sets none of the three (a direct client), so its published samples carry
`serverTs` = the worker's receipt stamp and no `receivedTs` extra.

## What the simulator exercises today

| Signal | Type | Behavior |
|---|---|---|
| `temperature-1` | number | A sine wave, `GOOD`, every poll. |
| `pressure-1` | null | Always `BAD` / `SENSOR_FAULT` — a worked example of reporting a failed read. |

Extend this table as `Device.java` starts talking to a real protocol — document every native type it
can produce and how each maps to the JSON value / quality above, the way the reference adapters
(`opcua-adapter/docs/reference/data-types.md`, `modbus-adapter`) document theirs.
