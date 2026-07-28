# Reference — Configuration

> This documents the generated scaffold; rewrite it as you build the component out.

Complete reference for every configuration option this scaffold understands. The machine-readable
source of truth is `config.schema.json`.

## Config source

| Platform | Default source | Example |
|----------|----------------|---------|
| `HOST` | `FILE <path>` | `-c FILE ./config.json` |
| `GREENGRASS` | `GG_CONFIG` | the deployment `ComponentConfiguration` |
| `KUBERNETES` | `CONFIGMAP` | a mounted ConfigMap directory (re-read on change) |

This component's own settings live under `component`; the sibling sections (`hierarchy`, `identity`,
`tags`, `messaging`, `credentials`, `logging`, `heartbeat`, `metricEmission`) are standard EdgeCommons
sections, owned by the canonical schema and not redeclared here.

## `component.token`

The UNS component token: the `{component}` segment of every topic this component publishes on, and
the `identity.component` field of every message envelope. UNS tokens are lower-kebab
(`<<BINNAME>>`); the Greengrass component name is the reverse-DNS `<<COMPONENTFULLNAME>>` and never
reaches the wire. Every shipped configuration and the recipe set it. Leave it set — without it the
library falls back to the short form of the component name, `<<COMPONENTNAME>>`, and the topics in
this documentation stop matching what the component publishes.

## `component` (global)

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `publish_interval` | integer | `3` | Seconds between the scaffold's publish tick (the demo status/metric/data/event quartet). |
| `message` | string | `"Hello world"` | The greeting carried in the scaffold's status body. `set-greeting` overrides it at runtime; this is the starting value. |

## `component.instances[]`

The scaffold ships with **no instances** — it runs as the implicit `main` instance. Declare
per-instance keys here as you add them:

| Key | Type | Required | Definition |
|-----|------|----------|-----------|
| `id` | string | yes | Unique instance id — the `{instance}` UNS topic segment, stamped into the envelope identity. |
| `publish_interval` | integer | no | Per-instance override of the global publish tick. |

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "factory-1" },
  "messaging": { "local": { "host": "localhost", "port": 1883 } },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "destination": "local" },
  "metricEmission": { "target": "log", "targetConfig": { "logFileName": "{ComponentFullName}.metric.log" } },
  "component": { "publish_interval": 3, "message": "Hello world" }
}
```

## Ignored / rejected keys

`config.schema.json` sets `additionalProperties: false` — an unknown key under `component` is a
startup error, not a silently ignored typo.
