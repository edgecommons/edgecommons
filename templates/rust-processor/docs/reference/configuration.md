# Reference — Configuration

*This documents the generated scaffold; rewrite it as you build the component out.*

Every option `<<COMPONENTNAME>>` itself understands. For *why*, see
[explanation.md](../explanation.md); for tasks, the [how-to guides](../how-to-guides.md); for
worked examples, [sample-configurations.md](../sample-configurations.md).

## Config source

The processor reads one JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`,
`GREENGRASS` → `GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under
`component`; the sibling sections (`tags`, `hierarchy`, `identity`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard `edgecommons` sections, owned by the canonical schema
and not redeclared here.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `tickMs` | integer | `10000` | Fallback stage-tick cadence for a route that does not set its own. A stage that emits on time rather than on arrival (a window, a batch, a debounce) produces its output on this cadence. |
| `maxQueue` | integer | `256` | Fallback queue bound: how many messages may be queued for a route before new ones are dropped and counted. |

## `component.instances[]` (one route each)

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `id` | string | **required** | Unique route id; must be lower-kebab (`^[a-z0-9]+(?:-[a-z0-9]+)*$`). |
| `subscribe` | array of string | `[]` | Topic filters this route consumes; wildcards allowed (`ecv1/+/+/+/data/#`). |
| `publishTopic` | string | **required** | Where the transformed result is published. |
| `target` | `local` \| `northbound` | `local` | Where the result goes: the device-local bus, or straight out to the northbound broker. |
| `pipeline` | array of stage | `[]` | The stages, in order (below). An empty pipeline is a pass-through republisher. |
| `maxQueue` | integer | `component.global.defaults.maxQueue` | Per-route override of the queue bound. |
| `tickMs` | integer | `component.global.defaults.tickMs` | Per-route override of the stage-tick cadence. |

### Stages (entries of `pipeline[]`)

A single-key object naming the stage and its arguments.

| Key | Fields | Definition |
|-----|--------|-----------|
| `fieldEquals` | `path` (string), `value` (any JSON) | Keep only messages whose dotted body path equals `value`; drop the rest. |
| `countPerTick` | *(none)* | Accumulate arrivals; emit one `{count, last}` rollup per tick. The stateful half of the trait — emits nothing on arrival, produces output in `on_tick`. |

Add your own stage here as you implement it in `src/proc.rs` — see the
[how-to guide](../how-to-guides.md#write-your-own-stage).

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity": { "site": "factory-1" },
  "messaging": { "local": { "type": "mqtt", "host": "localhost", "port": 1883 } },
  "metricEmission": { "target": "messaging" },
  "component": {
    "global": { "defaults": { "tickMs": 5000, "maxQueue": 512 } },
    "instances": [
      {
        "id": "rollup",
        "subscribe": ["ecv1/+/+/+/data/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/data/summary",
        "target": "local",
        "pipeline": [
          { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } },
          { "countPerTick": {} }
        ]
      }
    ]
  }
}
```

## Limitations

- `additionalProperties: false` throughout, so a typo'd key is caught at deploy time — extend the
  schema in the same change you extend `src/proc.rs`/`src/app.rs`.
- This scaffold's routes are homogeneous (every entry of `component.instances[]` is a `RouteConfig`)
  — there is no "kind" discriminator between routes the way a sink's `destination` is tagged.
