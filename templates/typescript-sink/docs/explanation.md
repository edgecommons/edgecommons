This documents the generated scaffold; rewrite it as you build the component out.

# Explanation — How this sink works, and why

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## The archetype

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

A sink is the last thing standing between data and its destination — everything upstream can retry
or route around a failure; a sink cannot, because there's nothing past it. That's why every step in
the ladder earns its place, and why none of them are optional.

## The two non-negotiable properties

- **Delivery is idempotent, to a stable key.** `keyFor` (`src/app.ts`) derives a deterministic key
  from the sink id, the arrival topic, and the message's UUID — the same message always resolves to
  the same key. This is what makes retry *safe*: a redelivery is an overwrite, not a duplicate. A
  sink that cannot retry without duplicating cannot retry at all.
- **`verify` runs before the source is released.** `deliverWithRetry` (`src/app.ts`) calls
  `destination.verify(item, delivered)` immediately after `deliver` resolves, and only returns
  (letting the caller consider the message handled) after verification succeeds. Trusting
  `deliver`'s resolution alone — without checking that what landed is what was sent — is how you
  end up having deleted the only copy of something that never actually arrived intact.

## Classify the failure: transient vs permanent

`DeliverError.transient` decides everything downstream of a failed `deliver`/`verify` call.
Getting it wrong is expensive in both directions: retrying a **permanent** failure (bad
credentials, a malformed key, a missing bucket) burns the retry budget and floods the log for
nothing, because it will fail identically forever; giving up on a **transient** one (a timeout, a
503, a full disk someone will empty) loses data a second attempt would have delivered. An
*unclassified* throw is treated as transient on purpose — a wrongly-permanent verdict is the more
expensive mistake.

## Retry with full jitter, and a time budget

`RetryConfig.delayMs` backs off exponentially with **full jitter** — without it, every component
that lost the same endpoint retries at the same instant, and an endpoint that's already struggling
gets hit by a synchronized thundering herd on every backoff boundary. `giveUpAfterMs` is
deliberately a **time budget, not an attempt count**: "twenty attempts" means something very
different at a 1-second backoff and at 15 minutes of backoff, while "keep trying for an hour" means
the same thing at every cadence — it's what an operator can actually reason about at 3 a.m.

## The event ladder is the contract with whoever is watching

`delivery-started` → `delivery-completed` **or** `delivery-failed` (carrying `willRetry`) → and, if
the budget runs out, `delivery-exhausted` at **Critical** severity. A sink that fails quietly is
indistinguishable from one that's idle; an operator must be able to tell "still trying" from "gave
up," and "gave up" must be loud — it's the one event severity in this scaffold that isn't `Info` or
`Warning`.

## One health, two surfaces, per destination

Each configured sink's destination is reported through `InstanceConnectivity` from the moment it's
**configured**, not from the moment it first succeeds — a destination nobody has delivered to yet
is not the same as one that's broken, so `DestHealth` starts `IDLE` (reported as `connected: true`)
and only moves to `BACKOFF`/`FAILED` once a real delivery attempt says so. The same `DestHealth`
value feeds both the `state` keepalive's `instances[]` array and the `sb`-style `status` verb — one
source, so a health dot and a status reply can never disagree.

## A note on scope

The shipped `LocalDestination` is intentionally small: write-to-temp-then-rename (atomic, so a
reader never observes a half-written file) and a deterministic key. It demonstrates the two
non-negotiable properties above without a network dependency. A real destination (S3, SFTP, HTTP,
a database) is more code, but the two properties it must uphold are exactly the same.
