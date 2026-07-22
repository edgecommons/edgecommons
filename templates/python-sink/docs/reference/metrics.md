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

## `sinkDeliveries`

One metric across every sink, flushed every 60 seconds (`METRIC_INTERVAL_SECS`).

Dimensions: none (aggregate across all sinks; add a `sink` dimension if you need per-sink breakdown,
and keep it bounded to your configured sink ids).

| Measure | Unit | Purpose |
|---|---:|---|
| `received` | Count | Items dequeued off the bus across every sink's subscription. |
| `delivered` | Count | Items delivered **and verified**. |
| `retried` | Count | Transient-failure retries attempted, across all sinks. |
| `exhausted` | Count | Items that gave up — permanent failure, or the time budget spent. **This is data that did not arrive**; treat a nonzero rate here as the alarm it is. |
| `dropped` | Count | Items dropped because a sink's bounded queue was full. |

## Adding your own metric

Follow the same shape: define once via `MetricBuilder.create(name).with_config(self._cm)` in
`__init__`, emit via `self._metrics.emit_metric(name, {...})` wherever the value changes. Keep
dimensions low-cardinality — a sink id or a result code, never a destination key, endpoint URL, or
raw error text; those belong in events, logs, or command replies.
