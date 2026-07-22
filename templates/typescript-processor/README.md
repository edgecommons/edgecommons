# <<COMPONENTNAME>>

A **processing component** (`<<COMPONENTFULLNAME>>`) written in TypeScript on top of the
`edgecommons` TypeScript library, generated from the EdgeCommons TypeScript processor template by
the `edgecommons` CLI.

A processor subscribes to messages, transforms them, and forwards the result.

```text
  subscribe(filter) ──► bounded queue ──► one loop per route ──► publish
                                             (Pipeline)          local | northbound
```

Each entry of `component.instances[]` is **one route**: topic filters, a pipeline of stages, and a
target. Routes are independent — one loop each — so a slow route cannot stall another, and per-key
state inside a stage needs no coordination.

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.ts` | Entry point: builds the `edgecommons` runtime from CLI args, runs the app. |
| `src/app.ts` | The route logic (unit-tested): parse, the self-echo guard, the bounded queue, the stats window, the identity restamp. |
| `src/runtime.ts` | The thin live-runtime seam: subscribe, run one loop per route, publish. Excluded from the coverage gate (needs a live runtime; validated by the deploy paths). |
| `src/proc.ts` | **The seam you implement**: `Processor` stages and the `Pipeline` that chains them. |
| `test/` | Vitest suites for the invariants below (`npm test`). |
| `config.schema.json` | The component's own config (`component.global` + one route per instance). |
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

# feed it something (any component publishing on ecv1/+/+/+/data/# will do) and watch the rollup
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/app/#' -v
```

## The seam: `src/proc.ts`

A stage takes a message and returns **zero or more** messages, so it can filter (return nothing), map
(return one), or fan out (return several). `0..N` covers all three without a special case — and it is
what lets `onTick` exist: a *stateful* stage (a window, a debounce, a batch) accumulates in `process`
and emits in `onTick`, so time-driven output is not a different mechanism from data-driven output.

Two demo stages ship: `fieldEquals` (a filter) and `countPerTick` (the stateful half). Replace them;
nothing in `src/proc.ts` is required by the library.

## What the archetype guarantees (and the tests that hold it there)

| Invariant | Why |
|---|---|
| **The self-echo guard** | A processor that publishes onto a class it also subscribes to will consume its own output, reprocess it, republish it, and saturate the device. The guard is identity-based, not topic-based: a filter can be widened in config by someone who never read this file, and the loop it opens is silent until the device falls over. |
| **The identity restamp** | What we publish is *ours*, not the producer's. Without it the fleet cannot tell who emitted a message — and the self-echo guard downstream cannot work either. |
| **A bounded queue that DROPS AND COUNTS** | An unbounded queue does not remove backpressure: it relocates the failure to the heap, and by the time you notice you have lost the ability to report it. A processor that silently discards messages is worse than one that crashes, so the drop is counted and reported on `processorThroughput`. |
| **A tick flows through the rest of the pipeline on the same pass** | A window closing in stage 1 is projected by stage 2 immediately, rather than waiting for the next message to shake it loose. A final tick on shutdown emits a half-full window instead of losing it. |
| **Raw `messaging()`, never `data()`** | The `data()` facade is for a component that *produces* readings: it mints its own topic from a signal id and imposes the `SouthboundSignalUpdate` body. A processor is **payload-agnostic** — it republishes what it was handed, on a topic its route names. Routing that through `data()` would rewrite both the topic and the body, which is exactly what a republisher must not do. |

## Configuration

`component.instances[]` is **one route per entry**; `config.schema.json` is the contract:

```json
{
  "component": {
    "global": { "defaults": { "tickMs": 10000, "maxQueue": 256 } },
    "instances": [
      {
        "id": "rollup",
        "subscribe": ["ecv1/+/+/+/data/#"],
        "publishTopic": "ecv1/gw-01/<<BINNAME>>/rollup/app/summary",
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

`target` is `local` (the device-local bus — the common case) or `northbound` (straight out to the
northbound broker). The example publishes on the `app` class because a rollup is not a
`SouthboundSignalUpdate`; publishing onto a class this route also subscribes to is safe *because* of
the self-echo guard, but naming a class that means what you are emitting is safer still. The reserved
classes (`state`, `metric`, `cfg`, `log`) are library-owned and rejected on direct publish.

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

## Docs and further reading

See [`docs/`](docs/) for the full Diátaxis set — a tutorial, how-to guides, an explanation of the
pipeline archetype, sample configurations, and reference pages for configuration, the messaging
interface, and metrics.

## Lockfile

This scaffold ships with no `package-lock.json` — a template cannot generate a *valid* lockfile
(the resolved graph depends on the dep-source and the moment you build), and doing so at scaffold
time would need network access, which the CLI deliberately avoids. Run `npm install` once, then
**commit `package-lock.json`** — `.gitignore` does not exclude it — so `npm ci` is reproducible in
CI and for every other contributor. `component validate` warns if it is missing.
