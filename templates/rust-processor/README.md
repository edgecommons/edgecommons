# <<COMPONENTNAME>>

A **processing component**: it subscribes to messages, transforms them, and forwards the result.

```text
  subscribe(filter) ──► bounded queue ──► one task per route ──► publish
                                             (Pipeline)          local | northbound
```

> Full docs: [`docs/README.md`](docs/README.md). This template ships without a `Cargo.lock` (the
> scaffold generates offline, without a toolchain or network); commit it after your first build.

## Run it

```bash
cargo run -- \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

The shipped example consumes every `data` message on the bus, keeps only `temperature-1`, counts
them, and republishes a rollup every 3 seconds.

## Where your code goes

`src/proc.rs`. A stage implements `Processor`:

```rust
pub trait Processor: Send {
    fn process(&mut self, m: ProcMsg) -> Out;                 // 0..N messages out
    fn on_tick(&mut self, now_ms: u64) -> Out { Out::new() }  // for stateful stages
}
```

A stage returns **zero or more** messages, so one trait covers a filter (returns nothing), a map
(returns one), and a fan-out (returns several). `on_tick` is what lets a *stateful* stage — a
window, a batch, a debounce — accumulate on arrival and emit on time.

Two stages ship as examples: `FieldEquals` (a filter) and `CountPerTick` (a rollup). Replace them.

## Configuration

Each entry of `component.instances[]` is **one route**. Routes are independent — one task each — so
a slow route cannot stall another, and per-key state inside a stage needs no lock.

```json
{
  "id": "rollup",
  "subscribe": ["ecv1/+/+/+/data/#"],
  "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/data/summary",
  "target": "local",
  "pipeline": [
    { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } },
    { "countPerTick": {} }
  ],
  "tickMs": 3000
}
```

`config.schema.json` is the contract for the above, and `edgecommons component validate` checks
against it — including the config your Kubernetes ConfigMap and Greengrass recipe deploy with.

## Three things not to remove

**The self-echo guard.** A processor that publishes onto a class it also subscribes to will consume
its own output, reprocess it, republish it, and saturate the device. `is_self_echo` drops anything
carrying our own identity.

**The identity restamp.** What this component publishes is *its own*, not the producer's. Without it
the fleet cannot tell who emitted a message — and the self-echo guard downstream cannot work either.

**The bounded queue.** When a route's queue is full, messages are **dropped and counted**, never
blocked on. An unbounded queue does not remove backpressure; it relocates the failure to the heap,
and by then you have lost the ability to report it. The `dropped` measure of the
`processorThroughput` metric is how you find out.

## Instance connectivity: a processor reports none

`App::new` registers an instance-connectivity provider that returns an empty list. A processor owns
no southbound links — its routes are message flows, not connections — so it has no instances to
report, and that is a real answer rather than a missing one: the `state` keepalive omits the
`instances[]` section, and the built-in `status` verb answers exactly what `ping` answers.

The seam is registered anyway, so it is visible the day this component grows a connection of its
own (an enrichment database, a model server). Return one `InstanceConnectivity` per connection:
`connected` is the normalized flag every console renders a health dot from, `state` is your own
vocabulary (`ONLINE` / `CONNECTING` / `BACKOFF` / `DISABLED`), and `attributes` is an open bag for
domain data. The comment in `App::new` shows the shape.

## Why this uses `messaging()` and not `data()`

The `data()` facade is for a component that *produces* readings: it mints its own topic from a
signal id and imposes the `SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it
republishes what it was handed, on a topic its route names. Routing that through `data()` would
rewrite both the topic and the body, which is exactly what a republisher must not do.
