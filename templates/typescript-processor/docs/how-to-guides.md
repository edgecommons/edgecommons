This documents the generated scaffold; rewrite it as you build the component out.

# How-to Guides

Recipes for specific tasks. Each assumes the component builds and runs (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Write your own stage

Implement `Processor` (`src/proc.ts`): `process(m)` returns zero, one, or many messages; add
`onTick(nowMs)` only if the stage emits on a timer rather than on arrival.

```ts
export class ThresholdCrossed implements Processor {
  private above = false;
  constructor(private readonly path: string, private readonly limit: number) {}

  process(m: ProcMsg): ProcMsg[] {
    const v = pluck(m.msg.body, this.path);
    const nowAbove = typeof v === "number" && v > this.limit;
    if (nowAbove === this.above) return []; // no transition, nothing downstream
    this.above = nowAbove;
    return [m];
  }
}
```

Then add a case to `StageConfig` and `buildStage` so it's reachable from config:

```ts
case "thresholdCrossed": {
  const { path, limit } = o.thresholdCrossed as { path?: unknown; limit?: unknown };
  if (typeof path !== "string") throw new Error("thresholdCrossed needs a `path`");
  if (typeof limit !== "number") throw new Error("thresholdCrossed needs a `limit`");
  return new ThresholdCrossed(path, limit);
}
```

and a matching entry to `config.schema.json`'s `$defs.stage`.

---

## Add a second route

Add another entry to `component.instances[]` — each is its own subscription, queue, pipeline, and
publish target, so a slow route never stalls another:

```jsonc
"instances": [
  { "id": "rollup", "subscribe": ["ecv1/+/+/+/data/#"], "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/app/summary",
    "pipeline": [ { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } }, { "countPerTick": {} } ] },
  { "id": "alarms", "subscribe": ["ecv1/+/+/+/evt/critical/#"], "publishTopic": "ecv1/gw-01/<<BINNAME>>/alarms/app/relay",
    "target": "northbound", "pipeline": [] }
]
```

An empty `pipeline` is a pass-through republisher — useful for a route that only needs the
self-echo guard and the identity restamp (e.g. relaying critical events northbound).

---

## Route on the arrival topic

`ProcMsg.topic` carries the topic a message arrived on. A stage can branch on it:

```ts
process(m: ProcMsg): ProcMsg[] {
  return m.topic.includes("/evt/critical/") ? [m] : [];
}
```

---

## Tune the queue and tick cadence

| You want… | Set |
|-----------|-----|
| A stateful stage to emit more/less often | `instances[].tickMs` (falls back to `global.defaults.tickMs`, default `10000`) |
| More buffering headroom before drops | `instances[].maxQueue` (falls back to `global.defaults.maxQueue`, default `256`) |

A full queue **drops and counts** rather than blocking or growing unbounded — see
[explanation.md](explanation.md#a-bounded-queue-that-drops-and-counts). Watch
`processorThroughput`'s `dropped` measure ([reference/metrics.md](reference/metrics.md)) to know
if you need a larger queue or a faster consumer.

---

## Republish onto a different target

`target: "local"` (the default) keeps the result on the device-local bus; `target: "northbound"`
sends it straight to the northbound broker instead — useful for a route that filters or aggregates
before forwarding off-box, so you don't push everything upstream unfiltered.

---

## Deploy to a platform

**HOST:** `node dist/main.js --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`

**Greengrass:** package per `gdk-config.json`/`recipe.yaml`; config comes from the deployment
(`--platform GREENGRASS -c GG_CONFIG`).

**Kubernetes:** build the image (`Dockerfile`), apply `k8s/` (config from a mounted ConfigMap,
identity from the Downward API).
