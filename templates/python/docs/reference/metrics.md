# Reference — Metrics

*This documents the generated scaffold; rewrite it as you build the component out.*

The scaffold emits one metric through the EdgeCommons metric service. With
`metricEmission.target: messaging`, it is published on the reserved UNS `metric` class:

```text
ecv1/{device}/<<BINNAME>>/main/metric/{metricName}
```

The component never writes reserved `metric` topics directly — it defines the metric through
`MetricBuilder`, so the same name, measures, and dimensions are used whether the target is
`messaging`, `log`, `cloudwatch`, or `prometheus`.

## `loopTicks`

Demonstrates that a metric is not just a single scalar: a monotonic counter and a gauge-like
elapsed-time measure, side by side.

Dimensions: `demo` (constant value `"scaffold"` — replace or remove it once you define a real
metric; a demo dimension is not something to carry into production code).

| Measure | Unit | Purpose |
|---|---:|---|
| `tickCount` | Count | Monotonic count of publish-loop iterations since start. Demonstrates a counter measure. |
| `uptimeSecs` | Seconds | Elapsed time since the app started. Demonstrates a gauge-like measure. |

## Adding your own metric

Define it once (in `__init__`, via `MetricBuilder.create(name).with_config(self._config_manager)`)
and emit it wherever the value changes (`self._metrics.emit_metric(name, {...})`). Keep dimensions
**low-cardinality** — an instance id, a result code, a bounded category — never a signal name,
address, endpoint URL, or raw error text; those belong in data messages, events, logs, or command
replies, not in metric dimensions (they would shred a fleet dashboard with unbounded cardinality).
