# Reference — Metrics

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTNAME>>` emits one metric, `sinkDeliveries`, through the EdgeCommons metric service
(`src/supervisor.rs`). With `metricEmission.target: messaging`, it publishes on the reserved UNS `metric`
class:

```text
ecv1/{device}/<<BINNAME>>/metric/sinkDeliveries
```

## `sinkDeliveries`

Cross-cutting delivery counters for the whole process, emitted every 60 seconds.

Dimensions: none beyond the library's own default component dimensions (this scaffold does not
dimension per-sink; add a `sink` dimension if you need per-sink breakdowns — keep it
low-cardinality, one value per configured sink).

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Messages accepted onto a sink's queue. |
| `delivered` | Count | Deliveries that completed and verified successfully. |
| `retried` | Count | Transient-failure retries attempted. |
| `exhausted` | Count | Deliveries that gave up — permanent failure, or the retry time budget spent. **This is the number that matters**: it is data that did not arrive. |
| `dropped` | Count | Messages dropped because a sink's queue was full. |

Each is reset to `0` after every emit (`AtomicU64::swap(0, ...)`), so the value is **per-interval**,
not cumulative.

## Adding your own

If a real destination backend needs its own measure (bytes transferred, a per-object latency), add
it to this same metric or define a new one — mirror the existing `Stats` counters (an `AtomicU64`
incremented where the event happens, drained and reset in `emit_metrics`). Keep every dimension
low-cardinality: `instance`/`sink id` is reasonable; a delivery key or error text is not.
