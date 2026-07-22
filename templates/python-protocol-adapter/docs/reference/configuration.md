# Reference — Configuration

*This documents the generated scaffold; rewrite it as you build the component out.*

Every configuration option this scaffold itself understands. For *why*, see
[explanation.md](../explanation.md); for the type/quality model, see [data-types.md](data-types.md);
for tasks, see the [how-to guides](../how-to-guides.md).

## Config source

The adapter reads one JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`,
`GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. Adapter settings live under `component`; the
sibling sections (`tags`, `hierarchy`, `identity`, `topic`, `messaging`, `logging`, `metricEmission`,
`heartbeat`) are standard edgecommons sections owned by the canonical schema and are **not**
redeclared in `config.schema.json`.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `defaults.pollIntervalMs` | integer | `5000` | Milliseconds between reads of each device in the adapter's poll loop. A subscribe-based protocol you implement can ignore it. |
| `healthThresholds.staleSignalSecs` | integer | `30` | A signal with no update for longer than this counts toward `southbound_health.staleSignals` (SOUTHBOUND.md §5). A signal that silently stops updating is otherwise indistinguishable from one that is simply not changing. |

## `component.instances[]` (a device)

A southbound adapter models **one device per instance** — how to reach it, how often to read it, and
what it is permitted to write. Each device runs on its own worker thread and publishes to its own UNS
data topics.

| Key | Type | Required | Definition |
|-----|------|----------|-----------|
| `id` | string | yes | Unique device id, lower-kebab (`^[a-z0-9]+(?:-[a-z0-9]+)*$`). The `{instance}` token of this device's UNS topics and the `instance` dimension of its metrics. |
| `adapter` | string | no (`"sim"`) | Which protocol backend to use. Matches `DeviceBackend.kind()` in `<<SNAKENAME>>/device.py`, and is published in every `SouthboundSignalUpdate`'s `device.adapter` field (e.g. `"modbus"`, `"opcua"`). |
| `connection` | object | yes | How to reach the device. Deliberately **open** — every protocol needs different keys (a unit id, a security policy, a slave address). This is the one place the adapter does not stay strict. |
| `connection.endpoint` | string | yes | The endpoint, in whatever form the protocol uses. Published in every `SouthboundSignalUpdate`'s `device.endpoint` field. |
| `pollIntervalMs` | integer | no | Per-device override of the read cadence. Falls back to `defaults.pollIntervalMs` (or `5000`). |
| `writes.allow` | string[] | no (`[]`) | Signal ids this device may write, matched on the **stable** `signal.id` (never a volatile index). Anything not listed is refused, whatever an `sb/write` command asks for. An empty list — the default — means the device is read-only. |

## Identity & the UNS device tree

`hierarchy.levels` names the enterprise tree, deepest (the device) last; `identity` supplies every
level's value **except** the last (always the resolved thing name). With the default (`["device"]`),
topics are `ecv1/{thing}/<<BINNAME>>/{instance}/...`.

```jsonc
"hierarchy": { "levels": ["site", "shop", "line", "device"] },
"identity":  { "site": "plant1", "shop": "assembly", "line": "5" }
// -> identity.path = "plant1/assembly/5/<thing>"
```

## Precedence

`pollIntervalMs` resolves: **device value ▸ `component.global.defaults`**.

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "site1" },
  "metricEmission": { "target": "messaging" },
  "component": {
    "global": {
      "defaults": { "pollIntervalMs": 5000 },
      "healthThresholds": { "staleSignalSecs": 30 }
    },
    "instances": [
      {
        "id": "device-1",
        "adapter": "sim",
        "connection": { "endpoint": "sim://device-1" },
        "writes": { "allow": ["temperature-1"] }
      }
    ]
  }
}
```

## Limitations

- The bundled `sim` backend implements exactly two signals (`temperature-1`, `pressure-1`) and
  accepts any write — it exists so the scaffold runs with no hardware, not as a protocol reference.
- `connection` has no schema beyond requiring `endpoint` — validate your protocol's own keys inside
  `DeviceBackend.connect()`, and raise `DeviceError(..., transient=False)` for a configuration
  mistake that will never fix itself by retrying.
