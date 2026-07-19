# Reference — Metrics

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTNAME>>` emits one metric, `processorThroughput`, through the EdgeCommons metric service
(`src/supervisor.rs`). With `metricEmission.target: messaging`, it publishes on the reserved UNS `metric`
class:

```text
ecv1/{device}/<<BINNAME>>/metric/processorThroughput
```

## `processorThroughput`

Cross-cutting counters for the whole process, emitted every 60 seconds.

Dimensions: none beyond the library's own default component dimensions (this scaffold does not
dimension per-route; add a `route` dimension if you need per-route breakdowns — keep it
low-cardinality, one value per configured route).

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Messages accepted onto a route's queue (after the self-echo guard). Helps confirm subscriptions are live. |
| `published` | Count | Messages successfully published downstream. |
| `dropped` | Count | Messages dropped because a route's queue was full. **Never let this stay invisible** — a processor that silently discards messages is worse than one that crashes. Climbing `dropped` means `maxQueue` is too small or the pipeline too slow for its input rate. |
| `errors` | Count | Publish failures. |

Each is reset to `0` after every emit (`AtomicU64::swap(0, ...)`), so the value is **per-interval**,
not cumulative.

## Adding your own

If a stage needs its own measure (a per-window average, a distinct error class), add it to this same
metric or define a new one — mirror the existing `Stats` counters (an `AtomicU64` incremented where
the event happens, drained and reset in `emit_metrics`). Keep every dimension low-cardinality:
`instance`/`route id` is reasonable; a signal name, a topic, or error text is not.
