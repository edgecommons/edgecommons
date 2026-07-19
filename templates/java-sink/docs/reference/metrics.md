# Reference — Metrics

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` emits one metric, `sinkDeliveries`, through the EdgeCommons metric service
(`MetricEmitter`). With `metricEmission.target: messaging`, it publishes on the reserved UNS `metric`
class:

```text
ecv1/{device}/<<BINNAME>>/metric/sinkDeliveries
```

## `sinkDeliveries`

A component-wide delivery summary, flushed every metrics interval and reset after each flush.

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Items accepted onto a sink's queue in the interval. |
| `delivered` | Count | Items delivered and verified in the interval. |
| `retried` | Count | Delivery attempts that failed transiently and were retried in the interval. |
| `exhausted` | Count | Items reported exhausted (permanent failure, or the retry time budget spent) — **this is data that did not arrive**; alert on this measure, not just on the log. |
| `dropped` | Count | Items dropped because a sink's queue was full. |

This is a single, component-wide family — it does not carry a per-sink dimension in the scaffold. If
you need per-sink delivery accounting, add a `sink` dimension (low-cardinality: the configured sink
ids) when you extend this metric.
