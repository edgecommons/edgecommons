# Sample Configurations

> This documents the generated scaffold; rewrite it as you build the component out.

Ready-to-adapt configurations for `<<COMPONENTNAME>>`. For the exhaustive option list see
[reference/configuration.md](reference/configuration.md); for topics/payloads see
[reference/messaging-interface.md](reference/messaging-interface.md).

## 1. The shipped demo route

`test-configs/config.json`:

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
        ],
        "tickMs": 10000
      }
    ]
  }
}
```

| Option | Effect |
|--------|--------|
| `id` | The `{instance}` UNS topic segment for this route; lower-kebab. |
| `subscribe` | Topic filters this route consumes; wildcards allowed. |
| `publishTopic` | Where the transformed result goes — a processor names its own output topic, unlike a component using `data()`. |
| `target` | `local` (device-local bus) or `northbound`. |
| `pipeline` | The stages, in order; an empty pipeline is a pass-through republisher. |
| `tickMs` | Per-route override of how often stateful stages are ticked. |

## 2. Pass-through republishing (no pipeline)

```jsonc
{
  "id": "mirror",
  "subscribe": ["ecv1/+/+/+/evt/#"],
  "publishTopic": "ecv1/gw-01/<<BINNAME>>/mirror/evt/mirrored",
  "target": "local",
  "pipeline": []
}
```

An empty `pipeline` republishes every non-self message unchanged (with the identity restamp) — useful
as a bridge between two topic shapes, or as a starting point before adding stages.

## 3. Two independent routes

```jsonc
"instances": [
  { "id": "alarms",  "subscribe": ["ecv1/+/+/+/evt/critical/#"],
    "publishTopic": "ecv1/gw-01/<<BINNAME>>/alarms/data/summary", "pipeline": [ { "countPerTick": {} } ] },
  { "id": "readings", "subscribe": ["ecv1/+/+/+/data/#"],
    "publishTopic": "ecv1/gw-01/<<BINNAME>>/readings/data/summary", "pipeline": [ { "countPerTick": {} } ] }
]
```

Each route gets its own worker thread and its own bounded queue — a burst on `readings` never stalls
`alarms`.

## 4. Send a route northbound

```jsonc
{
  "id": "rollup",
  "subscribe": ["ecv1/+/+/+/data/#"],
  "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/data/summary",
  "target": "northbound",
  "pipeline": [ { "countPerTick": {} } ]
}
```

Requires a `messaging.northbound` block in the top-level config (the dual-MQTT transport).

## Where settings resolve from (precedence)

`tickMs` and `maxQueue` resolve per-route ▸ `component.global.defaults` ▸ the built-in default
(`10000`ms / `256`).
