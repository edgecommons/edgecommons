# Reference — Metrics

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` emits metrics through the EdgeCommons metric service (`MetricEmitter`). With
`metricEmission.target: messaging`, they publish on the reserved UNS `metric` class:

```text
ecv1/{device}/<<BINNAME>>/metric/{metricName}
```

## Dimension model

Every family uses `instance` (the device id); `<<COMPONENTNAME>>Command` additionally uses `verb`
(the closed `sb/*` set) and `result` (`success`|`error`). Dimensions are deliberately
**low-cardinality only** — never a signal id, an address, an endpoint, or raw error text; those
belong in data messages, events, logs, or command replies.

## The Total/Interval counter convention

Every counter is emitted as a measure pair: `<name>Total` (monotonic since start) and
`<name>Interval` (since the previous emit; **reset on emit**). Gauges (`connectionState`) and
interval sums (the `*Ms` latencies/durations) are single measures.

## `southbound_health`

The canonical metric every southbound adapter in the ecosystem emits — the exact set below, no more,
no less.

Dimensions: `instance`.

| Measure | Unit | Purpose |
|---|---:|---|
| `connectionState` | Count | `1` connected, `0` disconnected. |
| `publishLatencyMs` | Milliseconds | How long the last publish batch took. |
| `pollLatencyMs` | Milliseconds | How long the last device read took. |
| `readErrors` | Count | Read failures in the interval. |
| `staleSignals` | Count | Signals with no update for longer than `healthThresholds.staleSignalSecs`. |
| `reconnects` | Count | Reconnects in the interval. |

## `<<COMPONENTNAME>>Connection`

The connect/reconnect lifecycle — a **worked example** of the operational-family pattern.

Dimensions: `instance`.

| Measure | Unit | Purpose |
|---|---:|---|
| `connectionState` | Count | `1` connected, `0` disconnected (duplicated here so the connection family is self-contained). |
| `connectAttemptsTotal` / `connectAttemptsInterval` | Count | Connect attempts, lifetime / this interval. |
| `connectFailuresTotal` / `connectFailuresInterval` | Count | Failed connect attempts. |
| `reconnectAttemptsTotal` / `reconnectAttemptsInterval` | Count | Re-establishments after a previous drop. |
| `connectionDropsTotal` / `connectionDropsInterval` | Count | Established sessions that were lost. |
| `connectedDurationMs` | Milliseconds | Accrued connected time since the last emit. |

## `<<COMPONENTNAME>>Command`

The `sb/*` command surface — the second worked example.

Dimensions: `instance`, `verb` (`sb/status`, `sb/read`, `sb/write`, `sb/signals`, `sb/browse`,
`sb/pause`, `sb/resume`, `reconnect`, `repoll`), `result` (`success`|`error`).

| Measure | Unit | Purpose |
|---|---:|---|
| `commandRequestsTotal` / `commandRequestsInterval` | Count | Requests for this `(verb, result)` combination. |
| `commandErrorsTotal` / `commandErrorsInterval` | Count | Failed requests (mirrors the `result:error` rows; present for a flat error-rate query). |
| `commandLatencyMs` | Milliseconds | Summed handling time in the interval. |

## Add your protocol's families here

`<<COMPONENTNAME>>Connection`/`Command` are generic — every adapter has them. A real protocol also
has an **inventory** (configured signals), a **poll/subscribe** path, and a **publish** path worth
measuring. See [How-to — Add your protocol's metric families](../how-to-guides.md#add-your-protocols-metric-families)
and the class header of `Metrics.java` for where to add `<<COMPONENTNAME>>Inventory` /
`<<COMPONENTNAME>>Poll` / `<<COMPONENTNAME>>Publish`.
