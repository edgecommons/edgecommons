This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Data Types

How a device reading becomes a published value. This is the page you delete-and-rewrite the most
completely once you implement a real protocol — the simulator has no type/scaling layer at all,
because it has no wire format to decode.

## What the simulator publishes

`SimSession.readSignals()` (`src/device.ts`) returns exactly two readings, every poll:

| `signalId` | `value` | `quality` | `qualityRaw` |
|---|---|---|---|
| `temperature-1` | a sine wave, `20.0 + 5.0 * sin(tick / 10)` | `GOOD` | `"OK"` |
| `pressure-1` | `null` | `BAD` | `"SENSOR_FAULT"` |

There is no register/byte/word-order decoding, no scale/offset transform, and no bit extraction —
the simulator hands `publishReadings` (`src/app.ts`) a plain JavaScript number (or `null`) directly. A real
backend's `DeviceSession.readSignals()` is where that translation happens.

## What a real protocol adds here

Most protocols distinguish a **native wire type** (a bit, a 16-bit register, a tag with a runtime
type code, …) from the **value** a consumer receives. When you replace the simulator, this page
should describe:

- The set of native types your protocol supports, and how each maps to a JSON `value`
  (number / boolean / string).
- Any byte- or word-order ambiguity (common in register-based protocols — see
  `modbus-adapter/docs/reference/data-types.md` for the canonical four-combination table:
  `wordOrder` × `byteOrder`).
- Scale/offset or unit conversions applied on read (and inverted on write), if your protocol
  carries raw counts rather than engineering units.
- How your protocol reports quality natively (if at all) — and what `qualityRaw` carries when you
  synthesize a `GOOD`/`BAD`/`UNCERTAIN` from it. `Quality.Good`/`Bad`/`Uncertain` (`src/device.ts`)
  are the three values `toLibQuality` maps onto the library's wire enum; a protocol with no native
  quality codes should mark a synthesized `GOOD` distinctly (e.g. `qualityRaw: "unspecified"`) so a
  consumer can tell it apart from a device-reported one.

## Identity: `signal.id` vs `signal.name` vs the protocol address

`Reading.signalId` is the **stable, canonical id** the rest of the fleet keys on — it must not
change across reconnects or reboots. `Reading.name` is a human label. Neither is the same as the
protocol-native address (a register number, a node id, a tag path): if your protocol needs one,
add it to the published body as `signal.address` (see `modbus-adapter`'s `u<unit>/<table>/<addr>`
pattern) rather than overloading `signalId` with it.
