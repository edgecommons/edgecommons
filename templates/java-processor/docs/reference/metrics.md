# Reference — Metrics

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` emits one metric, `processorThroughput`, through the EdgeCommons metric service
(`MetricEmitter`). With `metricEmission.target: messaging`, it publishes on the reserved UNS `metric`
class:

```text
ecv1/{device}/<<BINNAME>>/metric/processorThroughput
```

## `processorThroughput`

A component-wide throughput summary, flushed every metrics interval (60 s by default) and reset after
each flush.

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Messages accepted onto a route's queue in the interval (after the self-echo guard). |
| `published` | Count | Messages successfully published downstream in the interval. |
| `dropped` | Count | Messages dropped because a route's queue was full. **Never let this be invisible** — a processor that silently discards messages is worse than one that crashes. |
| `errors` | Count | Publish attempts that failed in the interval (also raises `evt/warning/publish-failed`). |

This is a single, component-wide family — it does not carry a per-route dimension in the scaffold. If
you need per-route throughput, add a `route` dimension (low-cardinality: the configured route ids)
when you extend this metric.
