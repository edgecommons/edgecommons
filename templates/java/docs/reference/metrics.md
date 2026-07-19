# Reference — Metrics

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` emits its demo metric through the EdgeCommons metric service (`MetricEmitter`).
With `metricEmission.target: messaging`, it publishes on the reserved UNS `metric` class:

```text
ecv1/{device}/<<BINNAME>>/metric/{metricName}
```

With the default `log` target it is written to a local file instead
(`metricEmission.targetConfig.logFileName`).

## `loopTicks`

The one metric this scaffold defines, via `MetricBuilder` — the sanctioned construction path (never
`new Metric(...)`, which is deprecated).

Dimensions: the library's default `coreName`/`component` dimensions, plus a custom `demo: "scaffold"`
dimension added to show `addDimension` in use.

| Measure | Unit | Purpose |
|---|---:|---|
| `tickCount` | Count | A monotonic counter incremented once per publish tick. |
| `uptimeSecs` | Seconds | Elapsed time since the component started. |

Replace `loopTicks` with metrics that mean something for your component's actual work — see
[How-to — Replace the demo surface](../how-to-guides.md#replace-the-demo-surface-with-your-own-business-logic).
