# Explanation — The Processor Archetype

> This documents the generated scaffold; rewrite it as you build the component out.

This page explains the shape of the scaffold so the route config and the pipeline contract make
sense as a whole. For a specific value or procedure, see the [reference](reference/) and the
[how-to guides](how-to-guides.md).

## One route per instance

Each entry of `component.instances[]` is one route: topic filters, a pipeline of stages, a publish
topic, and a target. Routes are independent — one worker thread each — so a slow route cannot stall
another, and the per-key state inside a stage needs no lock (a stage is *not* required to be
thread-safe, because exactly one thread ever calls it).

## A stage returns 0..N messages

A filter drops (returns nothing), a projection maps (returns one), an aggregator fans out (returns
several). `0..N` covers all three without a special case — and it is what lets `onTick(nowMs)` exist:
a stateful stage accumulates in `process` and emits in `onTick`, so time-driven output is not a
different mechanism from data-driven output. A tick flows through the *rest* of the pipeline on the
same pass, so a window closing in stage 1 is still projected by stage 2 immediately, rather than
waiting for the next message to shake it loose.

## Why a processor uses `getMessaging()` and not `getData()`

This is the mistake the archetype invites. The `data()` facade is for a component that *produces*
readings: it mints its own topic from a signal id and imposes the `SouthboundSignalUpdate` body. A
processor is **payload-agnostic** — it republishes what it was handed, on a topic its route names.
Routing that through `data()` would rewrite both the topic and the body, which is exactly what a
republisher must not do. So: raw `edgeCommons.getMessaging()`, and topics from config.

## The self-echo guard

A processor that publishes onto a class it also subscribes to will consume its own output, reprocess
it, republish it, and saturate the device. `isSelfEcho(...)` drops any message carrying our own
device + component identity. Do not remove it because "my route does not do that today" — a topic
filter is config, and config changes.

## The identity restamp

What we publish is **ours**, not the producer's. Every dispatched message is rebuilt with
`.withConfig(configManager)`, which stamps the envelope's `identity` block. Without the restamp the
fleet cannot tell who emitted a message — and the self-echo guard downstream cannot work either.

## The queue is bounded, and a drop is counted

Each route's queue is an `ArrayBlockingQueue(maxQueue)`, and the subscription handler `offer()`s into
it — never `put()`s. A full queue **drops and counts**; it does not block the transport's dispatch
thread. The `dropped` measure of `processorThroughput` is what makes that visible: a processor that
silently discards messages is worse than one that crashes.

## Why a final tick runs on shutdown

When the route worker stops, it drains whatever is left in the queue and runs one last tick before
exiting — so a half-full window is emitted rather than silently lost on a graceful stop.
