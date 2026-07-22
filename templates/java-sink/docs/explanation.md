# Explanation — The Sink Archetype

> This documents the generated scaffold; rewrite it as you build the component out.

This page explains why the archetype is shaped the way it is — the **ordering** is the archetype, and
every step earns its place. For a specific value or procedure, see the [reference](reference/) and
the [how-to guides](how-to-guides.md).

## 1. Deliver idempotently, to a stable key

The same item always lands at the same place, so a redelivery **overwrites** rather than duplicating.
This is what makes retry safe: a sink that cannot retry without duplicating cannot retry at all.
`keyFor(...)` derives the key from the sink id, the topic leaf, and the message's envelope uuid —
never from the clock, a counter, or a fresh random id, any of which would turn a retry into a
duplicate.

`LocalDestination` shows what a backend owes you here: it writes to a temp file and renames it into
place. A rename within a filesystem is atomic, so a reader never observes a half-written object, and
a crash mid-write leaves no corrupt artifact at the real key.

## 2. Verify before you confirm

`deliver` returning without throwing is not evidence. Releasing the source on that basis — without
checking what actually landed — is how you end up having deleted the only copy. `verify(item,
delivered)` runs **before** the confirmation, and a mismatch is a failure, not a warning.

## 3. Classify the failure

`DeliverException` is transient or permanent, and getting it wrong is expensive in both directions:
retrying a permanent error burns the retry budget and floods the log, while giving up on a transient
one loses data a second attempt would have delivered. When genuinely unsure, prefer transient.

## 4. Retry with full jitter, against a time budget

Exponential backoff, capped — and the delay is drawn uniformly from `[0, window)`, not fixed at the
window's edge. Without the jitter, every component that lost the same endpoint retries at the exact
same instant, and an endpoint that is already struggling gets a synchronized thundering herd on every
backoff boundary.

The give-up is a **time budget**, not an attempt count. "Twenty attempts" means something different at
1 s and at 15 min of backoff; "keep trying for an hour" means the same thing at every cadence, and is
what an operator can actually reason about.

## 5. Report every transition

A sink that fails quietly is indistinguishable from one that is idle. The event ladder
(`delivery-started` → `delivery-completed` / `delivery-failed` → `delivery-exhausted`) rides the UNS
`evt` class, and `delivery-exhausted` is deliberately **critical**: it is the one event that means
data did not arrive, and it is worth an operator's attention in a way a transient retry is not.

## Why the queue is bounded, and a drop is counted

The sink's queue is an `ArrayBlockingQueue(maxQueue)`, and the subscription handler `offer()`s into it
— never `put()`s. A full queue drops and counts; it does not block the transport's dispatch thread. A
consumer that silently discards messages under load is worse than one that visibly falls behind.
