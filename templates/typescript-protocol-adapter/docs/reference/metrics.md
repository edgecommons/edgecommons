This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Metrics

`<<COMPONENTNAME>>` emits health and operational metrics through the EdgeCommons metric service.
With `metricEmission.target: messaging`, they publish on the reserved UNS `metric` class:

```text
ecv1/{device}/{component}/metric/{metricName}
```

Every measure is defined through `MetricBuilder` (`src/metrics.ts`'s `familyDefs()`), so the same
names/units/dimensions are used regardless of target (`log`/`messaging`/`cloudwatch`/`prometheus`).

## Dimension model

Dimensions are intentionally low-cardinality: `instance`, `verb` (the closed `sb/*` verb set), and
`result` (`success`|`error`). **Never** dimension by signal name, address, endpoint, or error text
— those are unbounded and would shred a fleet dashboard.

## `southbound_health`

The canonical per-instance health metric every southbound adapter emits (SOUTHBOUND.md §5).

Dimensions: `instance`.

| Measure | Unit | Purpose |
|---|---:|---|
| `connectionState` | Count | `1` connected, `0` disconnected. Drives simple health alarms. |
| `publishLatencyMs` | Milliseconds | Time spent publishing the last poll's readings. |
| `pollLatencyMs` | Milliseconds | Time spent reading the last poll. |
| `readErrors` | Count | Read failures since the last emission. Reset on emit. |
| `staleSignals` | Count | Signals with no update for longer than `healthThresholds.staleSignalSecs`. |
| `reconnects` | Count | Reconnects (link drops) since the last emission. Reset on emit. |

## `<<COMPONENTNAME>>Connection`

The worked operational family for the connect/reconnect lifecycle.

Dimensions: `instance`.

| Measure | Unit | Purpose |
|---|---:|---|
| `connectionState` | Count | `1` connected, `0` disconnected (mirrors `southbound_health`). |
| `connectAttemptsTotal` / `connectAttemptsInterval` | Count | Connection attempts, monotonic / since last emit. |
| `connectFailuresTotal` / `connectFailuresInterval` | Count | Failed connection attempts. |
| `reconnectAttemptsTotal` / `reconnectAttemptsInterval` | Count | Re-establishments after a previous drop. |
| `connectionDropsTotal` / `connectionDropsInterval` | Count | Established sessions lost. |
| `connectedDurationMs` | Milliseconds | Time spent connected since the previous emission. |

## `<<COMPONENTNAME>>Command`

The worked operational family for the `sb/*` command surface.

Dimensions: `instance`, `verb` (`sb/status`, `sb/read`, `sb/write`, `sb/signals`, `sb/browse`,
`sb/pause`, `sb/resume`, `reconnect`, `repoll`), `result` (`success`|`error`).

| Measure | Unit | Purpose |
|---|---:|---|
| `commandRequestsTotal` / `commandRequestsInterval` | Count | Command invocations for this `(verb, result)`. |
| `commandErrorsTotal` / `commandErrorsInterval` | Count | Command invocations that errored. |
| `commandLatencyMs` | Milliseconds | Accumulated handler latency since the last emission. Reset on emit. |

## The Total/Interval counter convention

Every **counter** is emitted as a measure pair: `<name>Total` (monotonic since start) and
`<name>Interval` (since the previous emit of that family — reset on emit). Gauges
(`connectionState`) and interval sums (the `*Ms` latencies/durations) are single measures. This is
the same convention `modbus-adapter` and `ethernet-ip-adapter` use, so a fleet dashboard reads
every adapter the same way.

## Add your protocol's own families

`<<COMPONENTNAME>>Connection`/`Command` are generic — every adapter has them. Your protocol also
has an **inventory** (configured signals), a **poll/subscribe** path, and a **publish** path worth
measuring. See [../how-to-guides.md](../how-to-guides.md#add-your-protocols-own-metric-families).
