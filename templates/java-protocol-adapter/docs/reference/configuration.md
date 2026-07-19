# Reference — Configuration

> This documents the generated scaffold; rewrite it as you build the component out.

Complete reference for every configuration option this scaffold understands. For *why* these
settings exist, see [../explanation.md](../explanation.md); for task recipes, see the
[how-to guides](../how-to-guides.md); the machine-readable source of truth is `config.schema.json`.

## Config source

| Platform | Default source | Example |
|----------|----------------|---------|
| `HOST` | `FILE <path>` | `-c FILE ./config.json` |
| `GREENGRASS` | `GG_CONFIG` | the deployment `ComponentConfiguration` |
| `KUBERNETES` | `CONFIGMAP` | a mounted ConfigMap directory (re-read on change) |

Adapter settings live under `component`; the sibling sections (`hierarchy`, `identity`, `tags`,
`messaging`, `credentials`, `logging`, `heartbeat`, `metricEmission`) are standard EdgeCommons
sections, owned by the canonical schema and not redeclared here.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `defaults.pollIntervalMs` | integer | `5000` | Fallback read cadence for every device that does not override it. |
| `timeouts.connectMs` | integer | `5000` | How long a connect attempt may take before it is treated as failed. |
| `timeouts.reconnectBackoffMinMs` | integer | `1000` | The first reconnect backoff window. |
| `timeouts.reconnectBackoffMaxMs` | integer | `60000` | The reconnect backoff ceiling; backoff is jittered within the window. |
| `healthThresholds.staleSignalSecs` | integer | `30` | A signal with no update for longer than this counts toward `southbound_health.staleSignals`. |

## `component.instances[]` (one device per entry)

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | — | Unique device id (lower-kebab). The `{instance}` UNS topic segment and the `instance` metric dimension. |
| `adapter` | string | no | `"sim"` | Which `Device.DeviceBackend` services this device; published as `device.adapter`. |
| `connection` | object | yes | — | How to reach the device (below). |
| `pollIntervalMs` | integer | no | inherits `global.defaults` | Per-device override of the read cadence. |
| `writes.allow` | string[] | no | `[]` | Stable `signal.id`s this device may write. Empty = read-only. |

### `instances[].connection`

Deliberately **open** (`additionalProperties: true`) — every protocol needs different keys (a unit
id, a slave address, a security policy). The one key every backend can rely on:

| Key | Type | Required | Definition |
|-----|------|----------|-----------|
| `endpoint` | string | yes | The endpoint, in whatever form the protocol uses. Published in `device.endpoint`. |

Add your protocol's real connection keys here as you implement `Device.java` — see
[How-to — Implement a real device backend](../how-to-guides.md#implement-a-real-device-backend).

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "site1" },
  "messaging": { "local": { "host": "localhost", "port": 1883 } },
  "metricEmission": { "target": "messaging", "targetConfig": { "destination": "local" } },
  "component": {
    "global": {
      "defaults": { "pollIntervalMs": 5000 },
      "timeouts": { "connectMs": 5000, "reconnectBackoffMinMs": 1000, "reconnectBackoffMaxMs": 60000 },
      "healthThresholds": { "staleSignalSecs": 30 }
    },
    "instances": [
      {
        "id": "device-1",
        "adapter": "sim",
        "connection": { "endpoint": "sim://device-1" },
        "pollIntervalMs": 5000,
        "writes": { "allow": [] }
      }
    ]
  }
}
```

## Precedence

`pollIntervalMs` resolves per-device ▸ `component.global.defaults` ▸ the built-in default. Everything
else under `component.global` (`timeouts`, `healthThresholds`) is global-only in this scaffold.

## Ignored / rejected keys

`config.schema.json` sets `additionalProperties: false` everywhere except `connection` — an unknown
key anywhere else is a startup error, not a silently ignored typo.
