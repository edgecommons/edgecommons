This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Metrics

`<<COMPONENTNAME>>` emits one component-wide metric through the EdgeCommons metric service. With
`metricEmission.target: messaging`, it publishes on the reserved UNS `metric` class:

```text
ecv1/{device}/{component}/metric/{metricName}
```

## `sinkDeliveries`

Component-wide (not per-sink) counters, defined through `MetricBuilder` (`src/runtime.ts`) and
emitted every 60 seconds.

Dimensions: none beyond the library-injected component dimensions.

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Messages accepted by a sink's subscription. |
| `delivered` | Count | Deliveries that succeeded and were verified. |
| `retried` | Count | Transient failures that triggered a backoff-and-retry. |
| `exhausted` | Count | Deliveries that will never succeed — a permanent failure, or the retry budget spent. **This is the number that matters**: it's data that did not arrive. |
| `dropped` | Count | Messages received after shutdown began (not delivered, not retried). |

## Extending it

This scaffold's counters are intentionally coarse (component-wide, not per-sink) — a real component
with several sinks of very different reliability profiles will likely want to dimension by `sink`
(the sink `id`), the same low-cardinality-dimension discipline the reference adapters use
(instance/verb/result, never a message body field or a destination key). Add a `sink` dimension to
`MetricBuilder` in `src/runtime.ts` and thread the sink id through `Stats` (`src/app.ts`) if you
need per-sink visibility.
