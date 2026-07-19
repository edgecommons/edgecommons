# Explanation — How this sink is shaped, and why

*This documents the generated scaffold; rewrite it as you build the component out.*

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## A sink is the last thing standing between data and its destination

That framing drives every clause of the contract (`src/dest.rs`'s [`Destination`] trait):

- **`deliver` is the commit.** When it returns `Ok`, the item is live at its final, *stable* key —
  not staged, not pending.
- **The key is deterministic.** The same item always lands at the same place, so a redelivery is an
  **idempotent overwrite** rather than a duplicate. This is what makes retry safe at all: a sink that
  cannot retry without duplicating cannot retry.
- **`verify` runs before the source is released.** Trusting `deliver`'s `Ok` and letting go of the
  source without checking what actually landed is how you end up having deleted the only copy.
  `LocalDestination::verify` re-stats the file and compares byte counts; a real backend's `verify`
  should independently confirm content, not just re-trust its own write call.

`LocalDestination` demonstrates the two things every destination must get right regardless of
backend: **write-then-atomic-rename** (so a reader never observes a half-written object and a crash
leaves no corrupt artifact at the real key), and **deterministic keying** (`key_for` derives the key
from the sink id, the source topic's last segment, and the message's own UUID — never a counter or a
timestamp that could collide or drift).

## The delivery ladder, and why every rung reports something

[`deliver_with_retry`] moves through a fixed sequence, and every transition is both an event and a
health-state change — never one without the other:

1. **`delivery-started`** (info).
2. Either **`delivery-completed`** (info, with `attempts`/`elapsedMs`) — the terminal success — or
   **`delivery-failed`** (warning, carrying `willRetry: true` and the next backoff) for a transient
   failure that still has time budget, or **`delivery-exhausted`** (critical) for a permanent failure
   or a spent time budget.

A sink that fails quietly is indistinguishable from one that is idle — so every rung of the ladder
is reported, not just the terminal ones, and `delivery-exhausted` in particular is deliberately loud
(critical severity) because it means data that will never arrive.

## Classifying failure: transient vs. permanent

[`DeliverError::Transient`] (a timeout, a 503, a full disk someone will empty) is worth retrying;
[`DeliverError::Permanent`] (bad credentials, a malformed key, a missing bucket) will fail
identically forever. Getting this wrong is expensive in both directions: retrying a permanent
failure burns the time budget and floods the log for no benefit; giving up on a transient one loses
data a second attempt would have delivered. `LocalDestination` only ever produces `Transient` errors
(a permission problem or a full disk both *can* resolve without code changing) — a remote backend
(bad credentials, a bucket that does not exist) is where `Permanent` earns its keep.

## The time budget, not an attempt count

`giveUpAfterMs` bounds how long a delivery keeps retrying, not how many times. "Twenty attempts"
means something different at a 1-second backoff than at a 15-minute one; "keep trying for an hour"
means the same thing at every cadence — the number an operator can actually reason about when
deciding how long a real outage is tolerable before this sink should stop and page someone.

## Full jitter, and why it is not decoration

[`RetryConfig::delay`] picks a **uniform random point inside** the backoff window
(`[0, min(cap, base·2^attempt))`), not the window's edge. Without jitter, every sink instance that
lost the same endpoint at the same moment (a shared upstream outage) would retry in lockstep on every
backoff boundary — hitting an endpoint that is already struggling with a synchronized thundering
herd exactly when it is least able to absorb it.

## A sink's destinations are its instances

`App::run` registers one instance-connectivity provider entry **per configured sink**, moved by the
very same delivery ladder that emits the events above — the library reads it twice: it pushes the
sample into every `state` keepalive's `instances[]`, and it returns the same sample from the
built-in `status` command verb. [`DestState`] is this sink's own richer vocabulary
(`Idle`/`Online`/`Backoff`/`Failed`) precisely because the normalized boolean cannot distinguish
"still retrying" from "gave up" — and those are very different pages at 3 a.m. An **untried**
destination reports `Idle` (reachable, connected), not a broken one — nothing has failed yet because
nothing has been attempted yet.

[`Destination`]: ../src/dest.rs
[`deliver_with_retry`]: ../src/app.rs
[`DeliverError::Transient`]: ../src/dest.rs
[`DeliverError::Permanent`]: ../src/dest.rs
[`RetryConfig::delay`]: ../src/app.rs
[`DestState`]: ../src/app.rs
