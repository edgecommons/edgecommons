# Heartbeat System Documentation

## 1. Overview

The heartbeat is the library-owned liveness signal of every component (UNS-CANONICAL-DESIGN §4.3).
Each tick it does two independent things:

1. **State keepalive** — publishes a `state` envelope to the component's UNS state topic
   `ecv1/{device}/{component}/main/state` (rooted form `ecv1/{site}/{device}/…` when
   `topic.includeRoot` is true). Header `name` is `"state"`; the body is
   `{"status": "RUNNING", "uptimeSecs": <seconds since start>}`. On graceful shutdown
   (`GGCommons.shutdown()` / SIGTERM, or `EnhancedHeartbeat.stop()`) a best-effort
   `{"status": "STOPPED"}` state is published once.
2. **System measures** — the enabled measures (CPU, memory, disk, threads, files, fds) are emitted
   as a metric named **`sys`** through the normal metric subsystem, so they route to whatever
   `metricEmission` target is configured (log, messaging, CloudWatch, EMF, prometheus). See the
   [metric emission documentation](metric-emission.md).

The `state` UNS class is **reserved** (library-owned): components cannot publish to it directly —
the reserved-class publish guard on `MessagingClient` rejects it (`ReservedTopicError`); the
heartbeat publishes through the library-internal `MessagingClient._publish_reserved*` seam. See
[messaging.md](messaging.md).

## 2. Behavior

- The heartbeat starts automatically at component initialization and is **on by default**
  (`enabled: true`, every 5 seconds, keepalive to the local bus — D-U14/M11).
- The keepalive and the `sys` metric are each best-effort per tick: a failure in one never
  suppresses the other, and a failed tick never cancels future ticks.
- Configuration hot-reloads reschedule the loop (interval/enabled/measures/destination changes
  apply live).

## 3. Configuration

The legacy `heartbeat.targets[]` array (per-target topic/destination overrides) is **removed** —
hard cut. The section is now:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | boolean | `true` | Whether the heartbeat (keepalive + `sys` metric) runs. |
| `intervalSecs` | integer ≥ 1 | `5` | Tick interval in seconds. |
| `measures` | object | cpu+memory on | Which system measures the `sys` metric carries: `cpu`, `memory`, `disk`, `threads`, `files`, `fds` (booleans). |
| `destination` | `"local"` \| `"iotcore"` | `"local"` | Transport of the **state keepalive only**. The measures always route through the metric subsystem's own target. |

## 4. Sample Configurations

### Sample 1: Defaults (on / 5 s / local)

```json
{
    "heartbeat": {}
}
```

Publishes the `RUNNING` keepalive to `ecv1/{device}/{component}/main/state` every 5 seconds and
emits `sys` with CPU + memory through the configured metric target. (Omitting the section entirely
behaves the same.)

### Sample 2: Comprehensive measures at 30 s

```json
{
    "heartbeat": {
        "intervalSecs": 30,
        "measures": {
            "cpu": true,
            "memory": true,
            "disk": true,
            "threads": true,
            "files": true,
            "fds": true
        }
    }
}
```

### Sample 3: Keepalive to AWS IoT Core

```json
{
    "heartbeat": {
        "intervalSecs": 60,
        "destination": "iotcore"
    }
}
```

The state keepalive goes to IoT Core (QoS 1); the `sys` measures still follow `metricEmission`.

### Sample 4: Disabled

```json
{
    "heartbeat": {
        "enabled": false
    }
}
```

No keepalive, no `sys` metric (and no `STOPPED` state on shutdown — nothing was running).

## 5. Measure Details

### CPU Usage
- **Unit**: Percent
- **Range**: 0-100%
- **Description**: Current CPU utilization of the component process
- **Implementation**: Uses `psutil.Process.cpu_percent()`

### Memory Usage
- **Unit**: Megabytes (MB)
- **Description**: Resident Set Size (RSS) memory usage
- **Implementation**: Uses `psutil.Process.memory_info().rss / 1,000,000`

### Disk Usage
- **Units**: Gigabytes (GB)
- **Metrics**:
  - `disk_total`: Total disk space
  - `disk_used`: Used disk space
  - `disk_free`: Available disk space
- **Implementation**: Uses `shutil.disk_usage()` converted to GB

### Thread Count
- **Unit**: Count
- **Description**: Number of threads in the component process
- **Implementation**: Uses `len(psutil.Process.threads())`

### Open Files
- **Unit**: Count
- **Description**: Number of open file handles
- **Implementation**: Uses `len(psutil.Process.open_files())`

### File Descriptors
- **Unit**: Count
- **Description**: Number of file descriptors (Linux/Mac) or handles (Windows)
- **Implementation**: Uses `psutil.Process.num_fds()` or `psutil.Process.num_handles()`

## 6. Consuming heartbeats

Subscribe to the UNS state class, e.g. all components on all devices:

```
ecv1/+/+/+/state
```

or via the topic builder:

```python
from ggcommons import UnsClass, UnsScope

gg.uns().filter(UnsClass.STATE, UnsScope.all())   # -> "ecv1/+/+/+/state"
```

## 7. Usage in Code

The heartbeat is automatically initialized and started by `GGCommonsBuilder…build()` — no
additional code is required for basic operation. It reacts to configuration hot reloads through
the configuration-change listener system, so `enabled`/`intervalSecs`/`measures`/`destination`
changes apply without a restart.
