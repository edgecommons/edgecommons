# Explanation — How this processor is shaped, and why

*This documents the generated scaffold; rewrite it as you build the component out.*

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## A processor subscribes, transforms, forwards — nothing else

That is the whole archetype, and it lives in three types (`src/proc.rs`): [`ProcMsg`] (a message
plus the topic it arrived on), [`Processor`] (one stage — takes a message, returns zero or more), and
[`Pipeline`] (an ordered chain of stages, one route's transform). A stage returning **0..N** messages
(not `Option<ProcMsg>`) is what lets one trait cover a filter (drops, returns nothing), a map
(transforms, returns one), and a fan-out (returns several) — and it is what lets
[`Processor::on_tick`] exist at all: a *stateful* stage (a window, a debounce, a batch) accumulates on
`process` and emits on `on_tick`, so time-driven output is not a different mechanism from
data-driven output, just a different trait method on the same stage.

## One task per route, so state needs no lock

Each entry of `component.instances[]` is one route, and each route owns its `Pipeline` in a single
`tokio` task (`run_route`). That is deliberate: per-key state inside a stage is plain `&mut self`
with no `Mutex` anywhere, which is what makes a stateful stage cheap to write correctly — and a slow
or misbehaving route cannot stall another, since they share nothing but the process.

## Why `messaging()`, not `data()`

Worth reading twice, because it is the mistake this archetype invites. The `data()` facade is for a
component that *produces* readings: it mints its own topic from a signal id and imposes the
`SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it republishes what it was
handed, on a topic its route names in config. Routing that through `data()` would rewrite both the
topic and the body, which is exactly what a republisher must not do. So: raw `gg.messaging()`, and
topics from config (`publishTopic` per route).

## Two guards that are not optional

- **Self-echo.** A route that subscribes to a topic pattern its own output also matches will consume
  its own published messages, reprocess them, republish them, and loop until the device saturates.
  [`is_self_echo`] compares an inbound message's stamped `identity` against this component's own and
  drops anything that is ours.
- **Identity restamp.** [`dispatch`] rebuilds every outbound message with `MessageBuilder::new(...).
  from_config(config)` — what is published is *this component's own*, not the producer's. Without
  the restamp the fleet cannot tell who emitted a message, and the self-echo guard downstream (on
  whatever consumes this processor's output) cannot work either.

## The bounded queue: drop and count, never block

Each route's subscription handler does `tx.try_send(...)`, never `tx.send(...).await`. A full queue
means the pipeline cannot keep up with its input rate; the correct response is to **drop the new
message and count it** (`Stats::dropped`, surfaced in `processorThroughput`), not to block the
transport's dispatch task waiting for room. An unbounded queue does not remove backpressure — it
relocates the failure to the heap, and by the time you notice, you have already lost the ability to
report it. `dropped` climbing is the signal that `maxQueue` is too small or the pipeline too slow for
its input, and it is visible precisely because it was never allowed to be silent.

## Verify semantics: none of this archetype's own

A processor's contract ends at "I forwarded what my pipeline produced, or I counted why I could
not." There is no delivery-confirmation step here (compare the *sink* archetype, whose entire point
is verifying a delivery landed before releasing its source) — a processor's output is itself
transient traffic on the bus, consumed by whatever is downstream, not a terminal destination.

## Instance connectivity: a processor reports none

`App::new` registers an instance-connectivity provider that returns an empty list — a processor owns
no southbound links, its routes are message flows, not connections, so it has nothing to report.
That is a real answer, not a missing one: the `state` keepalive omits `instances[]`, and the built-in
`status` verb answers exactly what `ping` answers. The provider is registered anyway, so the seam is
visible the day this component grows a connection of its own (an enrichment database, a model
server) — see the comment in `App::new` for the shape.

[`ProcMsg`]: ../src/proc.rs
[`Processor`]: ../src/proc.rs
[`Processor::on_tick`]: ../src/proc.rs
[`Pipeline`]: ../src/proc.rs
[`is_self_echo`]: ../src/app.rs
[`dispatch`]: ../src/supervisor.rs
