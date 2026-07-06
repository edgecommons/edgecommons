# Heartbeat

The heartbeat is the library-owned liveness signal of every component
(UNS-CANONICAL-DESIGN §4.3). It is a `tokio` interval task that starts automatically
when the runtime is built and stops when `EdgeCommons` is dropped (RAII). Each tick it
does two independent things:

1. **State keepalive** — publishes a `state` envelope to the component's UNS state
   topic `ecv1/{device}/{component}/main/state` (rooted form `ecv1/{site}/{device}/…`
   when `topic.includeRoot` is true). Header `name` is `"state"`; the body is
   `{"status": "RUNNING", "uptimeSecs": <seconds since start>}`. On graceful shutdown
   (dropping `EdgeCommons` / SIGTERM) a best-effort `{"status": "STOPPED"}` state is
   published once.
2. **System measures** — the enabled measures are emitted as a metric named **`sys`**
   through the normal [metric subsystem](metric-emission.md), so they route to
   whatever `metricEmission` target is configured (log, messaging, CloudWatch,
   prometheus).

The `state` UNS class is **reserved** (library-owned): component code cannot publish
to it directly — the reserved-class guard on the messaging service rejects it with
`EdgeCommonsError::ReservedTopic`. The heartbeat publishes through the crate-private
`ReservedMessaging` seam (Rust is the one language where the seam is
compiler-enforced). See [messaging.md](messaging.md).

## Behavior

- **On by default**: `enabled: true`, every **5 seconds** (minimum 1), keepalive to
  the **local** bus (D‑U14/M11).
- Each tick reads the **live config snapshot**, so an interval/enabled/measures/
  destination change applied via hot-reload takes effect without a restart (the
  ticker is rebuilt on change).
- The keepalive and the `sys` metric are each best-effort per tick: a failure in one
  never suppresses the other, and a failed tick never cancels future ticks (fixing
  the Java C4/C5 class of bug).
- The first CPU sample reports `0.0` (CPU% needs a real time gap to measure over),
  matching psutil semantics; subsequent ticks measure over the interval.

## Configuration

The legacy `heartbeat.targets[]` array (per-target topic/destination overrides) is
**removed** — hard cut. The section is now:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | boolean | `true` | Whether the heartbeat (keepalive + `sys` metric) runs. |
| `intervalSecs` | integer ≥ 1 | `5` | Tick interval in seconds. |
| `measures` | object | cpu+memory on | Which system measures the `sys` metric carries (booleans, table below). |
| `destination` | `"local"` \| `"iotcore"` | `"local"` | Transport of the **state keepalive only**. The measures always route through the metric subsystem's own target. |

```json
{
  "heartbeat": {
    "intervalSecs": 30,
    "measures": { "cpu": true, "memory": true, "disk": true },
    "destination": "local"
  }
}
```

(Omitting the section entirely gives the defaults: `RUNNING` keepalive every 5 s to
`ecv1/{device}/{component}/main/state` + `sys` with CPU and memory.)

## Measures

Toggle collection with `heartbeat.measures`:

| Measure | Default | Source |
|---------|---------|--------|
| `cpu` | **true** | `sysinfo` (process CPU %, measured over the interval) |
| `memory` | **true** | `sysinfo` (resident memory, MB) |
| `disk` | false | `sysinfo` |
| `threads` | false | Linux `/proc`; Windows `windows-sys` (Toolhelp) |
| `files` | false | Linux `/proc/self/fd` |
| `fds` | false | Linux `/proc`; Windows `GetProcessHandleCount` |

Counters without a portable source are simply omitted on platforms that lack them.

## Consuming heartbeats

Subscribe to the UNS state class — all components on all devices:

```text
ecv1/+/+/+/state
```

or build the filter:

```rust
use edgecommons::uns::{UnsClass, UnsScope};

let filter = gg.uns().filter(UnsClass::State, &UnsScope::all())?; // "ecv1/+/+/+/state"
```
