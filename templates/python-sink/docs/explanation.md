# Explanation — How the sink archetype works, and why

*This documents the generated scaffold; rewrite it as you build the component out.*

This page is the mental model for the **sink** archetype. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## What a sink is

A sink is the last thing standing between data and its destination. It consumes work, delivers it
outward, and only then lets go of the source:

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

Every step in that ordering earns its place:

- **Deliver idempotently, to a stable key.** `key_for()` derives the key deterministically from the
  sink id, the topic leaf, and the message uuid — the same message always resolves to the same key,
  so a redelivery **overwrites**, it does not duplicate. A sink that cannot retry without duplicating
  cannot retry at all.
- **Verify before you confirm.** `Destination.verify()` runs after every `deliver()`, before the
  source is considered released. Trusting that `deliver` returned — without checking what actually
  landed — is how you end up having deleted the only copy of something that never arrived.
- **Classify the failure.** `DeliverError.transient` decides everything downstream: retrying a
  permanent error (bad credentials, a malformed key) burns the retry budget and floods the log;
  giving up on a transient one (a timeout, a full disk someone will empty) loses data a second
  attempt would have delivered.
- **Report every transition.** A sink that fails quietly is indistinguishable from one that is idle.
  `delivery-started` → `delivery-completed` **or** `delivery-failed` (repeating, with `willRetry`) →
  `delivery-exhausted` if the budget runs out. An operator must be able to tell "still trying" from
  "gave up", and gave-up must be loud (`delivery-exhausted` is a **critical alarm**, not an info log).

## The reference destination — small on purpose

`LocalDestination` demonstrates the two things every real backend must get right, and nothing else:
it **writes to a temp file and renames** (`os.replace` is atomic, so a reader never observes a
half-written object and a crash mid-write leaves no corrupt artifact at the real key), and it
**lands at a deterministic key** derived from the item's own key, not from anything time- or
process-specific. Replace it with S3, SFTP, HTTP, or whatever your destination is — the contract
(`kind`/`deliver`/`verify`) stays the same.

## Retry: full jitter, against a time budget

Exponential backoff with **full jitter**: the delay is a random point *inside* the window
`[0, min(cap, base·2^attempt))`, never the window's edge. The jitter is not decoration — without it,
every sink that lost the same endpoint retries at the same instant, and an endpoint that's already
struggling gets hit by a synchronized thundering herd on every backoff boundary.

The give-up is a **time budget, not an attempt count.** "Twenty attempts" means something different
at 1 second of backoff and at 15 minutes of backoff; "keep trying for an hour" means the same thing
at every cadence, and it is what an operator can actually reason about when deciding how long to
wait before treating something as lost.

## Instance connectivity — a sink's destinations *are* its instances

Unlike a service or a processor, a sink's `instance_connectivity()` is **not** an empty placeholder —
one `DestinationHealth` exists per configured destination from startup, before a single message
arrives, so a destination that is configured and unreachable is reported (`connected: false`,
`state: CONNECTING`) rather than silently absent. `connected` only becomes `True` once a delivery has
been **verified** — reporting a destination healthy because `deliver()` merely returned would be the
same mistake as releasing the source on an unverified write. `BACKOFF` (still retrying, within
budget) and `FAILED` (gave up; data did not arrive) are both `connected: False`, and deliberately
distinguishable — an operator needs to tell them apart, and a plain boolean cannot.

## The queue is bounded, and a full queue drops and *counts*

An unbounded queue does not remove backpressure; it relocates the failure to the heap, and by the
time you notice, you've lost the ability to report it. `_handler` uses `put_nowait`, never `put`: a
full queue increments the `dropped` measure of `sinkDeliveries` rather than blocking the transport's
dispatch thread.

## UNS addressing

Topics follow `ecv1/{device}/{component}/{instance}/{class}[/channel]`, built and validated by the
library. A sink's inbound filter is named by config; everything it publishes (`evt`, and the
library's `state`/`metric`) is minted through `gg.uns()` — never hand-written. The reserved classes
(`state`/`metric`/`cfg`/`log`) are library-owned and rejected on direct publish.
