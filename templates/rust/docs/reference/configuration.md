# Reference — Configuration

*This documents the generated scaffold; rewrite it as you build the component out.*

Every option `<<COMPONENTNAME>>` itself understands. For *why*, see
[explanation.md](../explanation.md); for tasks, the [how-to guides](../how-to-guides.md); for
worked examples, [sample-configurations.md](../sample-configurations.md).

## Config source

The component reads one JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`,
`GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under
`component`; the sibling sections (`tags`, `hierarchy`, `identity`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard `edgecommons` sections, owned by the canonical schema
and not redeclared here.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `publish_interval` | integer | `3` | Seconds between the scaffold's demo publish tick (the app-status/metric/data/event quartet it emits each loop). |

## `component.instances[]`

This scaffold declares a single instance, `main` — the instance its `data()`/`events()`/`metrics()`
facades are bound to.

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `id` | string | `"main"` | Unique instance identifier; the `{instance}` token of this instance's UNS topics and the envelope identity. |
| `publish_interval` | integer | `component.global.publish_interval` | Per-instance override of the tick cadence. |

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "messaging": { "local": { "type": "mqtt", "host": "localhost", "port": 1883 } },
  "metricEmission": { "target": "messaging" },
  "component": {
    "global": { "publish_interval": 5 },
    "instances": [ { "id": "main" } ]
  }
}
```

## Limitations

- `additionalProperties: false` throughout, so a typo'd key is caught at deploy time — extend the
  schema in the same change you extend `src/app.rs`.
- This scaffold is intentionally minimal: it has no destination, device, or pipeline concept of its
  own. If your component grows one, consider the matching archetype template (`protocol-adapter`,
  `sink`, `processor`) instead of building that shape from scratch here.
