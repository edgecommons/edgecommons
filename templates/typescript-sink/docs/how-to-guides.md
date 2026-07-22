This documents the generated scaffold; rewrite it as you build the component out.

# How-to Guides

Recipes for specific tasks. Each assumes the component builds and runs (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Implement a real destination

`src/dest.ts`'s `Destination` is the seam: `kind`, `deliver(item)`, `verify(item, delivered)`.
Whatever the backend, two properties are non-negotiable (see
[explanation.md](explanation.md#the-two-non-negotiable-properties)):

```ts
export class S3Destination implements Destination {
  readonly kind = "s3";
  async deliver(item: Item): Promise<Delivered> {
    // upload to a STABLE, deterministic key derived from item.key — a redelivery must overwrite,
    // never duplicate.
  }
  async verify(item: Item, delivered: Delivered): Promise<void> {
    // HEAD the object, compare size/etag against what deliver() reported — never trust deliver()'s
    // resolution alone.
  }
}
```

Then add a case to `DestinationConfig` and `buildDestination`, and a matching `oneOf` branch to
`config.schema.json`'s `$defs.destination`.

---

## Classify your destination's failures

`DeliverError.transientError(msg)` (retry — a timeout, a 503, a full disk someone will empty) vs
`DeliverError.permanent(msg)` (give up now — bad credentials, a malformed key, a missing bucket).
Getting this wrong is expensive in both directions: a wrongly-permanent verdict loses data a retry
would have delivered; a wrongly-transient one burns the retry budget on something that will never
succeed. An unclassified throw defaults to transient — see
[explanation.md](explanation.md#classify-the-failure-transient-vs-permanent).

---

## Tune the retry policy

```jsonc
"retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 }
```

| You want… | Set |
|-----------|-----|
| Faster first retry | lower `baseDelayMs` |
| A lower backoff ceiling | lower `maxDelayMs` |
| To keep trying longer/shorter before giving up | `giveUpAfterMs` — a **time budget**, not an attempt count |

Watch `sinkDeliveries.exhausted` — that's data that did not arrive.

---

## Add a second sink

Add another entry to `component.instances[]` — each gets its own subscription, destination, and
retry policy:

```jsonc
"instances": [
  { "id": "archive", "subscribe": "ecv1/+/+/+/data/#", "destination": { "type": "local", "path": "./out" } },
  { "id": "alarms",  "subscribe": "ecv1/+/+/+/evt/critical/#", "destination": { "type": "local", "path": "./alarms" },
    "retry": { "baseDelayMs": 500, "giveUpAfterMs": 600000 } }
]
```

---

## Change the source

The shipped scaffold's source is a **subscription** — it consumes messages matching `subscribe` and
delivers each one. If your source is a watched directory or a polled API instead, replace the
`messaging.subscribe(...)` call in the runtime seam (`src/runtime.ts`); everything downstream of
`deliverWithRetry` is unchanged, which is the point of the seam.

---

## Deploy to a platform

**HOST:** `node dist/main.js --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`

**Greengrass:** package per `gdk-config.json`/`recipe.yaml`; config comes from the deployment
(`--platform GREENGRASS -c GG_CONFIG`).

**Kubernetes:** build the image (`Dockerfile`), apply `k8s/` (config from a mounted ConfigMap,
identity from the Downward API).

---

## Observe health and status

- **Metric** `sinkDeliveries` — `received`, `delivered`, `retried`, `exhausted`, `dropped`.
- **State keepalive:** each sink's destination is reported as an instance from the moment it's
  *configured* — `{connected, state, attributes: {destination}}` — the same sample the built-in
  `status` verb answers on demand.
- **Events:** the delivery ladder — `delivery-started` → `delivery-completed` \| `delivery-failed`
  (`willRetry`) → `delivery-exhausted` (**Critical**).
