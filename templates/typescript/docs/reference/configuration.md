This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Configuration

Every configuration option this scaffold understands. For *why* these exist, see
[../explanation.md](../explanation.md); for tasks, [../how-to-guides.md](../how-to-guides.md).

## Config source

One JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`, `GREENGRASS` →
`GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under `component`
(this document); the sibling sections (`tags`, `hierarchy`, `identity`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard edgecommons sections owned by the canonical schema
(`schema/edgecommons-config-schema.json`) and are not redeclared here.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `publish_interval` | integer (seconds, ≥ 1) | `3` | Seconds between the scaffold's publish tick. Not read by the shipped demo code (`TICK_INTERVAL_MS` in `src/app.ts` is currently a fixed constant) — wire it up as you build out real logic; the key exists so `config.schema.json` demonstrates a validated, defaulted option. |

## `component.instances[]`

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | `"main"` | Unique instance identifier. The `{instance}` token of this instance's UNS topics and the identity stamped into every message it builds. |
| `publish_interval` | integer (seconds, ≥ 1) | — | Per-instance override of the global publish tick. |

The scaffold declares a single instance, `main`, which is the instance its `data()`/`events()`/
`metrics()` facades are bound to.

## Extending this schema

`additionalProperties: false` throughout, so a typo'd or unknown key is caught at deploy time
instead of silently ignored. Add your own keys here as you add configuration to the component, and
keep that discipline as you extend it.
