# Reference — Configuration

*This documents the generated scaffold; rewrite it as you build the component out.*

Every configuration option this scaffold itself understands. For *why*, see
[explanation.md](../explanation.md); for tasks, see the [how-to guides](../how-to-guides.md).

## Config source

The component reads one JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`,
`GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This scaffold's own settings live under
`component`; the sibling sections (`tags`, `hierarchy`, `identity`, `topic`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard edgecommons sections owned by the canonical schema
(`schema/edgecommons-config-schema.json`) and are **not** redeclared in `config.schema.json`.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `publish_interval` | integer | `2` | Seconds between the scaffold's publish tick — the `app`-status / metric / data / event quartet it emits each loop. |

## `component.instances[]`

The scaffold ships with **no instances** — it runs as the implicit `main` instance and reports no
southbound connections. Declare the per-instance keys your component reads as you add them; every
entry must at least carry:

| Key | Type | Required | Definition |
|-----|------|----------|-----------|
| `id` | string | yes | Unique instance identifier. The `{instance}` token of that instance's UNS topics (`ecv1/{device}/{component}/{instance}/...`) and the value stamped into the envelope identity. |
| `publish_interval` | integer | no | Per-instance override of the global publish tick, in seconds. |

## Identity & the UNS device tree

`hierarchy.levels` names the enterprise tree, deepest (the device) last; `identity` supplies every
level's value **except** the last (always the resolved thing name, from `-t`/Downward API/Greengrass
Thing name). With no `hierarchy` configured, the default is `["device"]` and topics are
`ecv1/{thing}/<<BINNAME>>/...` with no enterprise prefix.

```jsonc
"hierarchy": { "levels": ["site", "shop", "line", "device"] },
"identity":  { "site": "plant1", "shop": "assembly", "line": "5" }
// -> identity.path = "plant1/assembly/5/<thing>"
```

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "site1" },
  "heartbeat": { "enabled": true, "intervalSecs": 5, "measures": { "cpu": true, "memory": true }, "destination": "local" },
  "metricEmission": { "target": "messaging" },
  "tags": { "appId": "line5" },
  "component": {
    "global": { "publish_interval": 2 },
    "instances": []
  }
}
```

## Extending this schema

Add a property here every time `app/<<COMPONENTNAME>>.py` reads a new config key, and keep
`additionalProperties: false` on every object — a typo'd or unknown key should fail at deploy time,
not be silently ignored.
