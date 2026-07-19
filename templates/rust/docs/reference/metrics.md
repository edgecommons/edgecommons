# Reference — Metrics

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTNAME>>` emits one demo metric, `loopTicks`, through the EdgeCommons metric service
(`src/app.rs`). With `metricEmission.target: messaging`, it publishes on the reserved UNS `metric`
class:

```text
ecv1/{device}/<<BINNAME>>/metric/loopTicks
```

With the default `target: log` it writes to a local rotating log file instead; `cloudwatch` and
`prometheus` targets are also available (see the core library's platform docs).

## `loopTicks`

Demonstrates that a metric is not just a single scalar: one monotonic counter and one gauge-like
measure, emitted every `publish_interval` seconds.

Dimensions: a fixed `demo: "scaffold"` custom dimension (added via `add_dimension`) plus the
library's own default `coreName`/`component` dimensions.

| Measure | Unit | Purpose |
|---|---:|---|
| `tickCount` | Count | The loop's sequence number — a monotonic counter, never reset. |
| `uptimeSecs` | Seconds | Elapsed time since the component started — a gauge-like value, not accumulated. |

## Adding your own

Define a metric once (`MetricBuilder::create(name).with_config(&gg.config()).add_measure(...)
.add_dimension(...).build()`, then `metrics.define_metric(...)`), and emit it wherever the real event
happens — not necessarily on a fixed timer, as this demo does. Keep dimensions low-cardinality: a
fixed category label or an instance id is reasonable; a value that varies per message (a signal name,
a user id) is not.
