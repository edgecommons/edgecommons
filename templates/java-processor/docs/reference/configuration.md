# Reference — Configuration

> This documents the generated scaffold; rewrite it as you build the component out.

Complete reference for every configuration option this scaffold understands. The machine-readable
source of truth is `config.schema.json`; `RouteConfig.parse` enforces it at runtime, including
rejecting an unknown key.

## Config source

| Platform | Default source | Example |
|----------|----------------|---------|
| `HOST` | `FILE <path>` | `-c FILE ./config.json` |
| `GREENGRASS` | `GG_CONFIG` | the deployment `ComponentConfiguration` |
| `KUBERNETES` | `CONFIGMAP` | a mounted ConfigMap directory (re-read on change) |

Processor settings live under `component`; the sibling sections (`hierarchy`, `identity`,
`messaging`, `logging`, `heartbeat`, `metricEmission`, `credentials`) are standard EdgeCommons
sections, owned by the canonical schema and not redeclared here.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `tickMs` | integer | `10000` | How often stateful stages are ticked, for every route that does not override it. |
| `maxQueue` | integer | `256` | How many messages may be queued for a route before new ones are dropped and counted. |

## `component.instances[]` (one route per entry)

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | — | Unique route id (lower-kebab). The `{instance}` UNS topic segment. |
| `subscribe` | string[] | no | `[]` | Topic filters this route consumes. Wildcards allowed. |
| `publishTopic` | string | yes | — | The topic the transformed result is published on. |
| `target` | `local` \| `northbound` | no | `local` | Where the result goes. |
| `pipeline` | array of stage | no | `[]` | The stages, in order. An empty pipeline is a pass-through republisher. |
| `maxQueue` | integer | no | inherits `global.defaults` | Per-route override of the queue bound. |
| `tickMs` | integer | no | inherits `global.defaults` | Per-route override of the stage tick cadence. |

### `pipeline[]` (a stage)

A single-key object naming the stage and its arguments.

| Stage | Arguments | Behavior |
|---|---|---|
| `fieldEquals` | `path` (dotted body path), `value` (any JSON type) | Keep only messages whose value at `path` equals `value`; drop the rest. |
| `countPerTick` | *(none)* | Accumulate arrivals and emit one `{count, last}` rollup per tick. |

Add your own stage as a new key here, mirroring the `RouteConfig.buildStage` case you add in
`Stages.java` — see [How-to — Add a stage](../how-to-guides.md#add-a-stage).

## Complete example

```jsonc
{
  "hierarchy": { "levels": ["site", "device"] },
  "identity":  { "site": "factory-1" },
  "component": {
    "global": { "defaults": { "tickMs": 10000, "maxQueue": 256 } },
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

## Ignored / rejected keys

`config.schema.json` sets `additionalProperties: false` throughout, and `RouteConfig.parse` rejects
an unknown route key at runtime for the same reason — a typo'd route key is a mistake, not a no-op.
