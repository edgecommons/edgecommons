# Explanation — The Protocol-Adapter Archetype

> This documents the generated scaffold; rewrite it as you build the component out.

This page explains the shape of the scaffold so the seam, the config, and the command surface make
sense as a whole. For a specific value or procedure, see the [reference](reference/) and the
[how-to guides](how-to-guides.md).

## What an adapter is for

Industrial devices speak a protocol; the rest of the fleet speaks messages on a bus. An adapter is
the translator: it connects to one or more devices, reads the signals you configure, and republishes
their values as structured messages — and in the other direction, lets a client read or write a
signal on demand without knowing the protocol underneath.

```text
  connect -> poll -> publish SouthboundSignalUpdate -> report health
     ^                                                        |
     +------------- reconnect with backoff <-------------------+
```

## One device, one worker

Each entry of `component.instances[]` is one device, and each device gets its own worker thread with
its own connection lifecycle. A device that is slow to come up, or goes down mid-session, affects
only its own worker — not the others. The worker owns the device session; every command that must
touch it (a write, a browse, a reconnect) is routed through the worker's `Commands.DeviceControl`
seam under a lock, so a command can never race the poll loop on the same connection.

## The seam: `Device.java`

`DeviceSession` is one live connection; `DeviceBackend` opens sessions for a named `adapter` value.
Everything above the seam — the worker, the command surface, the metrics — is written against these
two interfaces and never learns a protocol's specifics. This is what makes replacing the simulator
with OPC UA, Modbus, or anything else a change confined to one file.

The boundary is deliberate: `Device.java` imports nothing from `com.mbreissi.edgecommons`. A backend
that reaches for the UNS or a metrics facade has leaked the seam — the mapping from protocol quality
to `GOOD | BAD | UNCERTAIN` happens one layer up, in the worker, not in the backend.

## Two planes: data and control

The message interface divides into a **data plane** (the continuous stream of `SouthboundSignalUpdate`
messages on the UNS `data` class, plus on-demand `sb/read`/`sb/write`) and a **control plane** (`sb/status`,
`sb/signals`, `sb/browse`, `reconnect`, `repoll`, `sb/pause`/`sb/resume`, the `evt` alarms, and
`southbound_health`). A telemetry consumer subscribes one wildcard, `ecv1/+/+/+/data/#`, and ignores
the rest; an operations console issues the `cmd/sb/*` queries and watches `ecv1/+/+/+/evt/#`.

## Why a failed read is published, not dropped

A signal the device could not currently read is still information: it means "I tried, and this is
what I know." The scaffold's simulated backend deliberately reports `pressure-1` as `BAD` on every
poll rather than omitting it, because a signal that silently stops updating is otherwise
indistinguishable from one that simply is not changing — which is exactly what `staleSignals` in
`southbound_health` is built to catch.

## Writes are allow-listed, checked before any device I/O

`writes.allow` is a list of stable `signal.id`s. `sb/write` checks every entry against it **before**
calling into the device — a refused entry never reaches `DeviceControl.write`. An adapter that
writes whatever it is asked is a control-system vulnerability, not a feature; an empty list (the
default) means read-only, which is the correct posture until you deliberately decide otherwise.

## Backoff is jittered on purpose

Reconnects use exponential backoff with full jitter (`Backoff.delayMs`), capped at a maximum. Without
the jitter, every adapter in a plant that lost the same PLC would retry on the exact same schedule —
a synchronized thundering herd hitting the device the instant it comes back.

## The operational-metrics pattern

`southbound_health` is the canonical, mandatory metric every adapter emits — exactly the measure set
in `docs/reference/metrics.md`, so a fleet dashboard reads every adapter the same way. On top of it,
`Metrics.java` ships two **worked examples** of the total/interval counter-pair pattern
(`<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command`) and marks where to add your own
protocol-specific families (an inventory count, a poll/subscribe rate, a publish rate) — see
[How-to — Add your protocol's metric families](how-to-guides.md#add-your-protocols-metric-families).

## The southbound contract

Every signal update uses one envelope shape, `SouthboundSignalUpdate`, shared by every adapter in the
ecosystem — the point being that a consumer written against the contract does not care whether the
data came from this adapter or any other. Identity is split into a stable `signal.id` (what a consumer
keys on) and whatever native address the protocol reports; quality is normalized so a consumer can
gate on it without a protocol-specific lookup table. Every message additionally carries a top-level
`identity` element (the enterprise hierarchy) stamped automatically by the library — the adapter never
hand-builds a topic or a body; it goes through the `data()`/`events()` facades.
