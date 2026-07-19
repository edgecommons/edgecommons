This documents the generated scaffold; rewrite it as you build the component out.

# Explanation — How this adapter works, and why

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## The southbound contract

A protocol adapter is a producer of the cross-language **southbound contract**: it publishes a
normalized `SouthboundSignalUpdate` envelope, exposes a read/write/browse command surface, and
emits `southbound_health` plus protocol-named operational metrics. The rest of the fleet sees the
same shape regardless of protocol — only `device.adapter`, the opaque `signal` identity, and any
protocol-specific metric families differ. This scaffold is deliberately generic (a two-signal
simulator); the reference adapters (`modbus-adapter`, `opcua-adapter`, `ethernet-ip-adapter`) are
what a fully protocol-specific implementation looks like.

## One loop per instance, one control channel

Every configured device runs its **own** connect → poll → publish loop
(`App.runDevice`/`runPolling` in `src/runtime.ts`). That loop also owns a `Mailbox<DeviceControl>` —
every `sb/*` verb that must touch the session or serialize with the poll (`write`, `readNow`,
`browse`, `pause`, `resume`, `reconnect`, `repoll`) is *sent* to the loop as a `DeviceControl` and
*confirmed* through the reply that rides it. This is why the command surface (`src/commands.ts`)
never touches a `DeviceSession` directly: most device protocols are a single request/response
channel, and two callers issuing requests at once would interleave into nonsense. Routing every
session-touching verb through the one loop that owns the session makes that structurally
impossible, not just a discipline you have to remember.

## The device seam (`src/device.ts`)

`DeviceSession` is the interface you implement once per protocol. The **boundary rule** is worth
enforcing in review: a backend knows protocols, not EdgeCommons — `src/device.ts` imports nothing
from `@edgecommons/edgecommons`. If a `DeviceSession` starts importing UNS/messaging/metrics types,
the seam has leaked, and the "swap the backend, keep everything above it" property is gone.

`BaseDeviceSession` supplies two honest defaults so a protocol that can't do better stays honest
rather than faking a capability: `readNamed` reads everything and filters (correct for any
backend), and `browse` rejects with `BrowseError.unsupported()` (a protocol with a fixed register
map and no discovery, like Modbus, should say so rather than return an empty page that looks like
"nothing here").

## Quality is not optional

Every reading carries a `quality` normalized to `GOOD | BAD | UNCERTAIN`, with the protocol-native
code preserved in `qualityRaw`. This is structural, not adapter discipline: `publishReadings`
(`src/app.ts`) passes every reading — including a failed one — through the `data()` facade, which
requires a quality. The simulator's `pressure-1` signal is always `BAD` on purpose, so you see from
the first run that a failed read is *reported*, not silently dropped: a signal that stops updating
would otherwise be indistinguishable from one that just isn't changing.

## Backoff with full jitter, and honest permanence

A failed connect attempt waits before retrying — exponentially, capped, with **full jitter**
(`Backoff.delayMs`). The jitter matters at fleet scale: without it, every instance that lost the
same upstream device reconnects on the same clock tick, and a device that is already struggling
gets a synchronized thundering herd on every backoff boundary. A `DeviceError` additionally
classifies **transient** (the link is down — retry) vs **permanent** (misconfiguration — a bad
endpoint, a rejected credential): a permanent failure backs off to the ceiling immediately, because
retrying it on the normal schedule will fail identically forever and only floods the log.

## The allow-list is checked before device I/O

`sb/write`'s allow-list check happens in `src/commands.ts`, entirely before a `DeviceControl` is
ever constructed. A refused write never reaches the device loop, let alone the protocol. An adapter
that writes whatever it's asked to write is a control-system vulnerability, not a feature — the
allow-list is the one thing standing between "a client asked" and "the device moved."

## Health: one source, several surfaces

`Health` (`src/app.ts`) is written by the device loop and read by three different consumers: the
metrics emitter (`southbound_health.connectionState`), the connectivity provider that feeds the
`state` keepalive's `instances[]`, and the `sb/status` command reply. One source means a health
dot, a metric, and a status reply can never disagree — there is no second copy of "is this device
connected" to drift out of sync.

## Metrics stay low-cardinality

`southbound_health` is the canonical per-instance health metric every adapter emits — dimensioned
only by `instance`. The two worked operational families (`<<COMPONENTNAME>>Connection`,
`<<COMPONENTNAME>>Command`) add `verb` and `result`, and nothing else. Never dimension by signal
name, address, endpoint, or error text: those are unbounded, and an unbounded dimension shreds a
fleet dashboard the first time someone points the adapter at a device with a thousand signals.

## A note on scope

This scaffold intentionally stops short of a reference adapter: no protocol-named families beyond
the two worked examples, no real backend, a one-page simulated browse. That is the deliberate line
between "minimal, canonical archetype" and "reference implementation" — see the sibling adapters
when you need the fuller pattern.
