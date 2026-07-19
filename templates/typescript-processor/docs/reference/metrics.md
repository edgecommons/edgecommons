This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Metrics

`<<COMPONENTNAME>>` emits one component-wide metric through the EdgeCommons metric service. With
`metricEmission.target: messaging`, it publishes on the reserved UNS `metric` class:

```text
ecv1/{device}/{component}/metric/{metricName}
```

## `processorThroughput`

Component-wide (not per-route) counters, defined through `MetricBuilder` (`src/app.ts`) and emitted
every 60 seconds.

Dimensions: none beyond the library-injected component dimensions.

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Messages accepted by a route's subscription (after the self-echo guard, before the queue). |
| `published` | Count | Messages successfully published downstream. |
| `dropped` | Count | Messages dropped because a route's queue was full. **This is the number that matters** — it's data that never reached the pipeline. See [../explanation.md](../explanation.md#a-bounded-queue-that-drops-and-counts). |
| `errors` | Count | Publish attempts that threw. Each also raises `evt/warning/publish-failed`. |

## Extending it

This scaffold's counters are intentionally coarse (component-wide, not per-route) — a real
component with several routes of very different volumes will likely want to dimension by `route`
(the route `id`), the same low-cardinality-dimension discipline the reference adapters use
(instance/verb/result, never a message body field). Add a `route` dimension to `MetricBuilder` in
`src/app.ts` and thread the route id through `Stats` if you need per-route visibility.
