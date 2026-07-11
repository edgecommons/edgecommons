# <<COMPONENTNAME>>

A **sink component** (`<<COMPONENTFULLNAME>>`) written in TypeScript on top of the `edgecommons`
TypeScript library, generated from the EdgeCommons TypeScript sink template by the `edgecommons` CLI.

A sink is the last thing standing between data and its destination. It consumes work, delivers it
outward, and only then lets go of the source.

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

**The ordering is the archetype.** Every step earns its place.

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.ts` | Entry point: builds the `edgecommons` runtime from CLI args, runs the app. |
| `src/app.ts` | The sinks: consume, key, deliver with retry, verify, confirm, report. |
| `src/dest.ts` | **The seam you implement**: `Destination` (`kind` / `deliver` / `verify`), plus a local-filesystem destination. |
| `test/` | Vitest suites for the invariants below (`npm test`). |
| `config.schema.json` | The component's own config (`component.global` + one sink per instance). |
| `test-configs/` | Sample `config.json` + `standalone-messaging.json` for local runs. |
| `recipe.yaml`, `gdk-config.json`, `build.sh` | Greengrass packaging. |
| `Dockerfile`, `k8s/`, `compose.yaml`, `supervisor/` | Container / Kubernetes / HOST packaging. |

## Develop & run locally (HOST platform, MQTT transport)

Start a local MQTT broker, then:

```bash
npm install
npm run build
node dist/main.js \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing

# anything published on ecv1/+/+/+/data/# lands under ./out/archive/... — and the event ladder
# (delivery-started / -completed / -failed / -exhausted) is on the evt class:
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/evt/#' -v
```

## The seam: `src/dest.ts`

`Destination` is the one thing you implement per backend — a filesystem, an object store, an HTTP
endpoint, a database. Everything above it (retry, verification, reporting) is written against the
interface and never learns what a bucket is.

The shipped `LocalDestination` is small, but it demonstrates the two things every destination must get
right: **write to a temp file and rename** (a rename is atomic, so a reader never observes a
half-written object and a crash mid-write leaves no corrupt artifact at the real key), and **derive
the key deterministically** so a redelivery overwrites rather than duplicates.

## What the archetype guarantees (and the tests that hold it there)

| Invariant | Why |
|---|---|
| **Deliver idempotently, to a STABLE key** | A redelivery overwrites; it does not duplicate. A sink that cannot retry without duplicating cannot retry at all. |
| **VERIFY before you confirm** | Trusting `deliver`'s success and releasing the source without checking what actually landed is how you end up having deleted the only copy. The source is released *after* verification, never before. |
| **Classify the failure: transient vs permanent** | Retrying a permanent error (bad credentials, a malformed key, a missing bucket) burns the budget and floods the log; giving up on a transient one (a timeout, a 503, a full disk someone will empty) loses data a second attempt would have delivered. An *unclassified* throw is treated as transient — a wrongly-permanent verdict loses data. |
| **Retry with exponential backoff + FULL JITTER, capped** | The jitter is not decoration: without it, every component that lost the same endpoint retries on the same instant, and an endpoint that is already struggling gets a synchronized thundering herd on every backoff boundary. |
| **The give-up is a TIME BUDGET, not an attempt count** | "Twenty attempts" means something different at 1 s and at 15 min of backoff. "Keep trying for an hour" means the same thing at every cadence, and it is what an operator can actually reason about. |
| **The event ladder** | `delivery-started` → `delivery-completed` \| `delivery-failed` (with `willRetry`) → `delivery-exhausted` (**Critical**). A sink that fails quietly is indistinguishable from one that is idle; an operator must be able to tell "still trying" from "gave up", and gave-up must be loud. |

The counters behind them ride the `sinkDeliveries` metric: `received`, `delivered`, `retried`,
`exhausted`, `dropped`. `exhausted` is the number that matters — it is data that did not arrive.

## Where the work comes from

This scaffold's source is a **subscription**: it consumes messages off the bus and delivers each one.
That is the common case. If your source is a watched directory or a polled API, replace the subscribe
call in `App.run` — everything downstream of `deliverWithRetry` is unchanged, which is the point of
the seam.

## Configuration

`component.instances[]` is **one sink per entry**; `config.schema.json` is the contract:

```json
{
  "component": {
    "global": { "defaults": { "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 } } },
    "instances": [
      {
        "id": "archive",
        "subscribe": "ecv1/+/+/+/data/#",
        "destination": { "type": "local", "path": "./out" },
        "retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 }
      }
    ]
  }
}
```

`additionalProperties: false` throughout, so a typo'd key is caught at deploy time instead of being
silently ignored. Add a `destination` variant to the schema as you implement one in `src/dest.ts`.

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Deploy to Greengrass

Packaged with the **GDK** using `gdk-config.json` and `recipe.yaml`; the custom build (`build.sh`)
runs `npm install` + `npm run build` and stages a ZIP artifact (`dist/` + `node_modules/` +
`package.json`).

```bash
gdk component build
gdk component publish
```

## Deploy to Kubernetes

Generated only when KUBERNETES is a selected target. Build the image from `./Dockerfile`, make it
available to the cluster, point `image:` at it, then apply the manifests:

```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest    # or: kind load docker-image ...
kubectl apply -f k8s/
```

With `--platform auto` the library detects KUBERNETES from the ServiceAccount token, reads config
from the mounted ConfigMap (hot-reloaded on `kubectl apply`), uses the MQTT transport from that same
ConfigMap, and resolves identity from the Downward API — so the Deployment needs no args.

## The edgecommons dependency

`package.json` depends on the `edgecommons` library via a `file:` path dependency (filled in at
generation time, `--dep-source local`, the default). Build the sibling library first (`npm run build`
in `core/libs/ts`), since a `file:` dependency on a TypeScript package needs its `dist/` present.
Regenerate with `--dep-source registry` to depend on the published package instead.
