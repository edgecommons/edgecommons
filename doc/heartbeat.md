# Heartbeat

The heartbeat is a `tokio` interval task that periodically collects system metrics
and emits them, giving operational visibility into component health. It starts
automatically when the runtime is built and stops when `GgCommons` is dropped (RAII).

## Behavior

- Emits on the interval from `heartbeat.intervalSecs` (default 5, minimum 1).
- Each tick reads the **live config snapshot**, so an interval change applied via
  hot-reload takes effect without a restart (the ticker is rebuilt on change).
- Each tick body is wrapped so a transient failure logs and the next tick still
  fires — the heartbeat cannot be permanently killed by one error (fixing the Java
  C4/C5 class of bug).
- The first CPU sample reports `0.0` (CPU% needs a real time gap to measure over),
  matching psutil semantics; subsequent ticks measure over the interval.

## Measures

Toggle collection with `heartbeat.measures`:

| Measure | Default | Source |
|---------|---------|--------|
| `cpu` | false | `sysinfo` (process CPU %, measured over the interval) |
| `memory` | false | `sysinfo` (resident memory, MB) |
| `disk` | false | `sysinfo` |
| `threads` | false | Linux `/proc`; Windows `windows-sys` (Toolhelp) |
| `files` | false | Linux `/proc/self/fd` |
| `fds` | false | Linux `/proc`; Windows `GetProcessHandleCount` |

Counters without a portable source are simply omitted on platforms that lack them.

## Targets

`heartbeat.targets` is a list; each entry has a `type` and optional `config`:

- **`metric`** — emit through the [metric service](metric-emission.md). No extra
  config. The metric target then decides where the data lands (log, messaging,
  CloudWatch, …).
- **`messaging`** — publish directly over the messaging service. `config`:
  - `destination`: `ipc`/`local` or `iotcore`
  - `topic`: template, e.g. `heartbeat/{ThingName}/{ComponentName}`

  (Messaging-target heartbeats require an available messaging service — STANDALONE
  mode today; GREENGRASS IPC is Phase 2.)

## Sample configuration

```json
{
  "intervalSecs": 5,
  "measures": { "cpu": true, "memory": true, "disk": false },
  "targets": [
    { "type": "metric" },
    { "type": "messaging", "config": { "destination": "ipc", "topic": "heartbeat/{ThingName}/{ComponentName}" } }
  ]
}
```
