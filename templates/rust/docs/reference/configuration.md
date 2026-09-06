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

## `component.token`

The UNS component token: the `{component}` segment of every topic this component publishes on, and
the `identity.component` field of every message envelope. UNS tokens are lower-kebab
(`<<BINNAME>>`); the Greengrass component name is the reverse-DNS `<<COMPONENTFULLNAME>>` and never
reaches the wire. Every shipped configuration and the recipe set it. Leave it set — without it the
library falls back to the short form of the component name, `<<COMPONENTNAME>>`, and the topics in
this documentation stop matching what the component publishes.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `publish_interval` | integer | `3` | Illustrative field. The demo loop uses `TICK_INTERVAL` (10 seconds) in `src/app.rs`; it does not read this field. |

## `component.instances[]`

The sample configuration declares an instance named `main`. The demo publishes through component-
scope facades and does not bind to this instance. Add explicit `gg.instance(id)` handles for
instance-scoped work.

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `id` | string | `"main"` | Unique instance identifier; the `{instance}` token of this instance's UNS topics and the envelope identity. |
| `publish_interval` | integer | `component.global.publish_interval` | Illustrative setting; no per-instance timer reads it in this scaffold. |

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "messaging": { "local": { "type": "mqtt", "host": "localhost", "port": 1883 } },
  "metricEmission": { "target": "messaging" },
  "component": {
    "token": "<<BINNAME>>",
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
