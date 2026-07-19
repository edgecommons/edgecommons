This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Metrics

`<<COMPONENTNAME>>` emits one demo metric through the EdgeCommons metric service. With
`metricEmission.target: messaging`, it publishes on the reserved UNS `metric` class:

```text
ecv1/{device}/{component}/metric/{metricName}
```

The default target is `log` (a rotating local file), not `messaging` — see
[../reference/configuration.md](configuration.md) and
[../sample-configurations.md](../sample-configurations.md#2-publishing-metrics-onto-the-uns-instead-of-a-log-file)
to switch it.

## `loopTicks`

Defined through `MetricBuilder` (`src/app.ts`) and emitted every tick (`TICK_INTERVAL_MS`, 10 s).

Dimensions: `demo` (constant `"scaffold"`) plus the library-injected component dimensions.

| Measure | Unit | Purpose |
|---|---:|---|
| `tickCount` | Count | A monotonic counter — increments every tick. Demonstrates a simple counter measure. |
| `uptimeSecs` | Seconds | Seconds since `run()` started. Demonstrates a gauge-like measure alongside a counter. |

## Extending it

Delete `loopTicks` once you have a real metric to emit — it exists purely so a freshly generated
component has something on a dashboard. Follow the same shape (`MetricBuilder.create(name)
.withConfig(config).addMeasure(...).addDimension(...).build()`, then `metrics.emitMetric(name,
values)`) for your own metrics, and keep dimensions low-cardinality — see the reference adapters'
metrics pages for what "low-cardinality" means in practice once your component grows connections or
a command surface of its own.
