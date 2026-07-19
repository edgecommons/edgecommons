This documents the generated scaffold; rewrite it as you build the component out.

# Explanation — How this processor works, and why

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## The archetype: subscribe, transform, forward

A processor is one shape, three types (`src/proc.ts`): a `ProcMsg` (a message plus the topic it
arrived on), a `Processor` (one stage — takes a message, returns zero or more), and a `Pipeline`
(an ordered chain of stages, one route's output feeding the next stage's input). `0..N` covers
filtering (return nothing), mapping (return one), and fan-out (return several) without a special
case for any of them — and it's what lets a *stateful* stage exist: it accumulates in `process`
(returning nothing) and emits in `onTick`, so time-driven output is not a different mechanism from
data-driven output.

## One route, one loop, no lock

Each entry of `component.instances[]` is a **route**: subscribe filters, a pipeline, a target. Each
route gets its own `BoundedQueue` and its own loop (`App.runRoute`). Because a route's pipeline only
ever runs on that one loop, per-key state inside a stage (a window's accumulator, a debounce timer)
needs no coordination — there is only ever one caller.

## Why `messaging()`, never `data()`

Worth internalizing, because it's the mistake this archetype invites. `data()` is for a component
that *produces* readings: it mints its own topic from a signal id and imposes the
`SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it republishes whatever it was
handed, on a topic its own route config names. Routing that through `data()` would rewrite both the
topic and the body, which is exactly what a republisher must not do. So `src/app.ts` uses raw
`gg.messaging()` and reads `publishTopic` straight from config.

## The self-echo guard

A processor that publishes onto a class it also subscribes to will consume its own output,
reprocess it, republish it, and loop forever until the device falls over. `isSelfEcho` guards
against this — and it is **identity-based, not topic-based** on purpose: a topic filter can be
widened later by someone who never read this file (`ecv1/+/+/+/app/#` instead of a narrower one),
and the loop it opens would otherwise be silent until it wasn't. Comparing against our own
`identity.path`/`identity.component` catches it regardless of how the filter is written.

## The identity restamp

`App.dispatch` rebuilds the outgoing message through `MessageBuilder`, which stamps *our own*
identity — never the identity riding the message we consumed. What we publish is ours, not the
producer's: without the restamp, the fleet cannot tell who actually emitted a message, and the
self-echo guard downstream (on whatever consumes our output) cannot work either.

## A bounded queue that drops and counts

`BoundedQueue.push` returns `false` rather than growing when it's full — a queue that grows without
bound does not remove backpressure, it relocates the failure to the heap, and by the time you
notice you've lost the ability to report it. So a full queue drops the newest message and counts
it (`Stats.dropped`, published on `processorThroughput`): a processor that silently discards
messages is worse than one that crashes, because a crash is loud and a silent drop is not.

## A tick flows through the rest of the pipeline on the same pass

`Pipeline.run`'s optional `nowMs` argument calls `onTick` on every stage **after** its data pass,
and joins whatever it emits into the same batch flowing downstream. A window closing in stage 1 is
projected by stage 2 immediately, rather than waiting for the next arriving message to shake it
loose. `App.runRoute` also calls this one final time on shutdown, so a half-full window is emitted
rather than silently lost when the process stops.

## A note on scope

This scaffold ships two demo stages (a filter and a stateful rollup) — enough to exercise both
halves of the `Processor` interface, not a library of transforms. Real pipelines usually need more:
a projection, a join across two subscriptions, a debounce. Add them to `src/proc.ts` following the
same shape.
