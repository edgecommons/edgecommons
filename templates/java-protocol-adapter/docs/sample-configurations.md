# Sample Configurations

> This documents the generated scaffold; rewrite it as you build the component out.

Ready-to-adapt configurations for `<<COMPONENTNAME>>`. Each is a valid config document; the prose
after it explains what every option does. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for the topic/message contract see
[reference/messaging-interface.md](reference/messaging-interface.md).

> **How config reaches the adapter.** It reads one JSON document from the `-c/--config` source,
> which defaults by platform: `HOST` → `FILE`, `GREENGRASS` → `GG_CONFIG` (the deployment),
> `KUBERNETES` → `CONFIGMAP` (a mounted, hot-reloaded directory). Adapter settings live under
> `component`; the sibling sections (`hierarchy`, `identity`, `messaging`, `logging`, `heartbeat`,
> `metricEmission`) are the canonical EdgeCommons config.

## 1. Minimal local run (HOST + MQTT), one simulated device

The shipped `test-configs/<<COMPONENTNAME>>.json`:

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

Run it:

```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT \
  -c FILE ./test-configs/<<COMPONENTNAME>>.json -t my-thing
```

| Option | Effect |
|--------|--------|
| `component.global.healthThresholds.staleSignalSecs` | A signal with no update for longer than this counts toward `southbound_health.staleSignals`. |
| `component.instances[].id` | The `{instance}` UNS topic segment and the `instance` metric dimension. Must be lower-kebab. |
| `component.instances[].adapter` | Which `Device.DeviceBackend` services this device; `sim` is the only one this scaffold ships. |
| `connection.endpoint` | Opaque to the framework — whatever your backend needs; the sim backend only checks it is non-empty. |
| `pollIntervalMs` | How often the worker reads this device. |
| `writes.allow` | Empty = read-only. This is the secure-by-default posture. |

## 2. Allow one signal to be written

```jsonc
"instances": [
  {
    "id": "device-1",
    "adapter": "sim",
    "connection": { "endpoint": "sim://device-1" },
    "writes": { "allow": ["temperature-1"] }
  }
]
```

Now `sb/write` on `temperature-1` succeeds (a confirmed, per-entry result); any other signal id is
still refused with `WRITE_NOT_ALLOWED` — matching is exact against the stable `signal.id`, never a
wildcard.

## 3. Multiple devices

Because each device is an independent worker, one deployment bridges several by listing several
`instances`:

```jsonc
"component": {
  "global": { "defaults": { "pollIntervalMs": 5000 } },
  "instances": [
    { "id": "device-1", "adapter": "sim", "connection": { "endpoint": "sim://device-1" } },
    { "id": "device-2", "adapter": "sim", "connection": { "endpoint": "sim://device-2" }, "pollIntervalMs": 1000 }
  ]
}
```

Every `sb/*` request now requires `"instance"` in its body — with exactly one device configured the
field is optional and defaults to it; with two or more, a missing id is `BAD_ARGS` and an unknown one
is `NO_SUCH_INSTANCE`.

## 4. Greengrass v2 deployment (IPC)

On `--platform GREENGRASS` there is no messaging block and no config file; config comes from the
deployment's `ComponentConfiguration` (the same shape as `recipe.yaml`'s
`ComponentConfiguration.DefaultConfiguration.ComponentConfig`), and the transport is IPC.

```bash
java -jar <<JARNAME>>-1.0.0.jar --platform GREENGRASS -t my-thing
# package/publish: gdk component build && gdk component publish
```

The `component` object is identical in shape to HOST — only the config source and transport differ.

## Where settings resolve from (precedence)

`pollIntervalMs` resolves per-device ▸ `component.global.defaults` ▸ the built-in default (`5000`).
`healthThresholds.staleSignalSecs` and the reconnect `timeouts` are global-only in this scaffold —
extend `config.schema.json` if you need a per-device override.
