# Sample Configurations

*This documents the generated scaffold; rewrite it as you build the component out.*

The scaffold ships one working config, `test-configs/<<COMPONENTNAME>>.json`, plus the MQTT
`standalone-messaging.json` for local HOST runs. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for the seam model see
[explanation.md](explanation.md).

## `test-configs/<<COMPONENTNAME>>.json` — one simulated device

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "site1" },
  "tags": { "appId": "dev" },
  "logging": { "level": "INFO" },
  "metricEmission": { "target": "messaging", "targetConfig": { "destination": "local" } },
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
        "pollIntervalMs": 5000,
        "writes": { "allow": [] }
      }
    ]
  }
}
```

**What each option does at runtime**

| Option | Effect |
|--------|--------|
| `hierarchy.levels` / `identity` | Places the component in the UNS enterprise tree. The last level (`device`) is always the resolved thing name, from `-t`. |
| `metricEmission.target: messaging` | Routes `southbound_health` and the operational metric families onto the UNS `metric` class, so you can see them on the bus without a CloudWatch/Prometheus target configured. |
| `component.global.defaults.pollIntervalMs` | Fallback poll cadence for any device that doesn't override it. |
| `component.global.healthThresholds.staleSignalSecs` | How long a signal can go without an update before it counts toward `southbound_health.staleSignals` (SOUTHBOUND.md §5). |
| `instances[].id` | This device's UNS instance token (`device-1`) and the `instance` dimension of its metrics. Must be lower-kebab. |
| `instances[].adapter` | Which protocol backend to use — `"sim"` matches `SimBackend.kind()` in `<<SNAKENAME>>/device.py`; a real deployment names your protocol here (e.g. `"modbus"`, `"opcua"`). |
| `instances[].connection.endpoint` | The device address, in whatever form the protocol uses. Published in every `SouthboundSignalUpdate`'s `device.endpoint` field. The simulator only checks that this key is non-empty. |
| `instances[].pollIntervalMs` | Per-device override of the read cadence. |
| `instances[].writes.allow` | The write allow-list, by stable `signal.id`. Empty — the default — means this device is read-only; `sb/write` refuses every entry with `WRITE_NOT_ALLOWED` before it ever reaches the device. |

## `test-configs/standalone-messaging.json` — the HOST/MQTT broker

```json
{ "messaging": { "local": { "host": "localhost", "port": 1883, "clientId": "<<BINNAME>>-local" } } }
```

Passed as the `--transport MQTT <path>` argument, independent of the `-c FILE ...` component config.

## Enabling writes

Add the signal id to `writes.allow`:

```jsonc
"writes": { "allow": ["temperature-1"] }
```

Now `sb/write` for `temperature-1` reaches the simulator (which accepts any write) instead of being
refused — see the [tutorial](tutorial.md#6-try-a-write-and-see-it-refused).

## Adding a second device

Add another entry to `component.instances[]` with its own `id`, `adapter`, `connection`, and
`writes.allow`. Once more than one device is configured, every `sb/*` command **must** include
`instance` in its body — see [explanation.md](explanation.md#instance-routing).
