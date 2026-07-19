# Reference — Metrics

*This documents the generated scaffold; rewrite it as you build the component out.*

The adapter emits health and operational metrics through the EdgeCommons metric service. With
`metricEmission.target: messaging`, metrics are published on the reserved UNS `metric` class:

```text
ecv1/{device}/<<BINNAME>>/metric/{metricName}
```

The adapter never writes reserved `metric` topics directly — it defines metrics through
`MetricBuilder`, so the same names, measures, and dimensions are used whether the target is
`messaging`, `log`, `cloudwatch`, or `prometheus`.

## The Total/Interval counter convention

Every **counter** measure is emitted as a pair: `<name>Total` (monotonic since start) and
`<name>Interval` (since the previous emit of that family — **reset on emit**). **Gauges**
(`connectionState`) and interval **sums** (the `*Ms` latencies/durations) are single measures. This is
the same convention the reference adapters (`modbus-adapter`, `ethernet-ip-adapter`) use, so a fleet
dashboard reads every adapter the same way.

## Dimension model

Dimensions are intentionally low-cardinality: `instance`, `verb` (the closed `sb/*` verb set), and
`result` (`success` | `error`) — and nothing else. Signal names, addresses, endpoint URLs, and raw
error text are **not** metric dimensions; use data messages, events, logs, or command replies for
those details.

## `southbound_health`

The **exact SOUTHBOUND.md §5 measure set** — every adapter in the ecosystem emits this, whatever its
protocol.

Dimensions: `instance`.

| Measure | Unit | Purpose |
|---|---:|---|
| `connectionState` | Count | `1` connected, `0` disconnected. Drives simple health alarms. |
| `publishLatencyMs` | Milliseconds | Time spent publishing samples during the last poll. |
| `pollLatencyMs` | Milliseconds | Time spent reading during the last poll. |
| `readErrors` | Count | Read errors observed during the reporting interval. |
| `staleSignals` | Count | Signals with no update for longer than `component.global.healthThresholds.staleSignalSecs`. |
| `reconnects` | Count | Reconnect events (link drops) observed during the reporting interval. |

## `<<COMPONENTNAME>>Connection`

The worked operational family for the connect/reconnect lifecycle. Named from the component so a
fleet view can tell one adapter's connection health from another's.

Dimensions: `instance`.

| Measure | Unit | Purpose |
|---|---:|---|
| `connectionState` | Count | `1` connected, `0` disconnected (gauge, not a pair). |
| `connectAttemptsTotal` / `connectAttemptsInterval` | Count | Initial connection attempts. |
| `connectFailuresTotal` / `connectFailuresInterval` | Count | Failed initial connection attempts. |
| `reconnectAttemptsTotal` / `reconnectAttemptsInterval` | Count | Re-establishments after a previous drop. |
| `connectionDropsTotal` / `connectionDropsInterval` | Count | Live links marked down by a broken read. |
| `connectedDurationMs` | Milliseconds | Time spent connected since the previous emission. |

## `<<COMPONENTNAME>>Command`

The worked operational family for the `sb/*` command surface.

Dimensions: `instance`, `verb` (`sb/status`, `sb/read`, `sb/write`, `sb/signals`, `sb/browse`,
`sb/pause`, `sb/resume`, `reconnect`, `repoll`), `result` (`success` | `error`).

| Measure | Unit | Purpose |
|---|---:|---|
| `commandRequestsTotal` / `commandRequestsInterval` | Count | Command handler invocations for this `(verb, result)` combination. |
| `commandErrorsTotal` / `commandErrorsInterval` | Count | Handler invocations that returned a coded error. |
| `commandLatencyMs` | Milliseconds | Accumulated handler latency for this combination since the previous emission. |

## Add your protocol's families here

`<<SNAKENAME>>/metrics.py` signposts the extension point: add
`<<COMPONENTNAME>>Inventory`/`Poll`/`Publish` families next to the two above for protocol-specific
detail (configured-signal counts, poll cycles, samples good/bad, batch flushes, …). Register each new
family in `family_defs()` and pre-define it in `DeviceMetrics.define_all()` — the rest of the pattern
(record → drain → emit) is copy-shaped from `<<COMPONENTNAME>>Command`. See
`modbus-adapter/modbus_adapter/metrics.py` and
`ethernet-ip-adapter/crates/ethernet-ip-adapter/src/metrics.rs` for the full worked set.
