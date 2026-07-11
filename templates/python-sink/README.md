# <<COMPONENTNAME>>

A **sink component** (`<<COMPONENTFULLNAME>>`) written in Python on top of the `edgecommons` Python
library, generated from the EdgeCommons Python sink template by the `edgecommons` CLI. It gives you
the library's standard CLI contract, configuration, logging, messaging, metrics and heartbeat — so
you write only the destination, in [`app/dest.py`](app/dest.py).

## What a sink is

A sink is the last thing standing between data and its destination. It consumes work, delivers it
outward, and only then lets go of the source.

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

**The ordering is the archetype**, and every step earns its place:

* **Deliver idempotently, to a stable key.** A redelivery overwrites; it does not duplicate. A sink
  that cannot retry without duplicating cannot retry at all. `key_for()` derives the key
  deterministically from the sink id, the topic leaf and the message uuid.
* **Verify before you confirm.** Trusting that `deliver` returned and releasing the source without
  checking what actually landed is how you end up having deleted the only copy. `verify()` runs
  first, every time.
* **Classify the failure.** Retrying a permanent error burns the budget; giving up on a transient one
  loses data a second attempt would have delivered. `DeliverError` carries `transient`, and nothing
  else decides whether the sink retries.
* **Report every transition.** A sink that fails quietly is indistinguishable from one that is idle.

| Path | What it is |
|------|-----------|
| `main.py` | Entry point — builds `EdgeCommons` and starts the app. |
| `app/<<COMPONENTNAME>>.py` | The wiring: sinks, subscriptions, the bounded queue, the retry loop, the event ladder, metrics. |
| `app/dest.py` | **Where your code goes** — the `Destination` abstraction, the local destination, the error taxonomy, the retry policy, the config parser. Pure logic: it does not import the library, so it is unit-testable on its own. |
| `tests/` | `pytest` tests for the invariants above. `python -m pytest` — no broker needed. |
| `config.schema.json` | The config this component itself understands (`component.global` + each `component.instances[]` entry). |
| `test-configs/` | A working `config.json` + the MQTT `standalone-messaging.json` for local HOST runs. |

## The destination

`Destination` is the seam: `kind()`, `deliver(item)`, `verify(item, delivered)`. Implement it once per
backend; everything above it — retry, verification, reporting — is written against the abstraction and
never learns what a bucket is.

`LocalDestination` ships as the worked example, and it is small on purpose: it demonstrates the two
things every backend must get right. It **writes to a temp file and renames** (`os.replace` is
atomic, so a reader never observes a half-written object and a crash mid-write leaves no corrupt
artifact at the real key), and it **lands at a deterministic key** so a redelivery overwrites rather
than duplicating. Add your backend to `app/dest.py`'s `build_destination()` **and** to
`config.schema.json`'s `destination` variants — the two are one contract.

## Retry: full jitter, against a time budget

Exponential backoff with **full jitter**: the delay is a random point *in* the window
`[0, min(cap, base * 2^attempt))`, not the window's edge. The jitter is not decoration — without it,
every component that lost the same endpoint retries on the same instant, and an endpoint that is
already struggling is hit by a synchronized thundering herd on every backoff boundary.

The give-up is a **time budget, not an attempt count**. "Twenty attempts" means something different
at 1 s and at 15 min of backoff; "keep trying for an hour" means the same thing at every cadence, and
it is what an operator can actually reason about.

## The event ladder

An operator must be able to tell "still trying" from "gave up", and gave-up must be loud.

| Event | Severity | When |
|---|---|---|
| `delivery-started` | Info | the item was dequeued and delivery began |
| `delivery-completed` | Info | delivered **and verified**; the source is released here, never before |
| `delivery-failed` | Warning | a transient failure; carries `willRetry` and `nextAttemptInMs` |
| `delivery-exhausted` | **Critical (alarm)** | permanent failure, or the time budget is spent — **this is data that did not arrive** |

They ride the sink's own instance token: `ecv1/{device}/{component}/{sink}/evt/{severity}/{type}`.
Watch them all with `ecv1/+/+/+/evt/#`, or just the alarms with `ecv1/+/+/+/evt/critical/#`.

Alongside them, the `sinkDeliveries` metric reports `received`, `delivered`, `retried`, `exhausted`
and `dropped` each interval. `dropped` is the queue-full counter: the queue is bounded, and a full
queue **drops and counts** rather than blocking the transport's dispatch thread.

## Configuration

```json
{
  "component": {
    "global": {
      "defaults": {
        "retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 },
        "maxQueue": 256
      }
    },
    "instances": [
      {
        "id": "archive",
        "subscribe": "ecv1/+/+/+/data/#",
        "destination": { "type": "local", "path": "./out" },
        "retry": { "baseDelayMs": 1000, "giveUpAfterMs": 3600000 }
      }
    ]
  }
}
```

`id` is the sink's UNS instance token (lower-kebab) **and** the prefix of every destination key it
writes — so it must be stable: change it and every redelivery lands somewhere new. Unknown keys are
rejected rather than ignored: a config knob that silently does nothing is the worst kind of bug to
find in the field.

## Run locally (HOST platform, MQTT transport)

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`), or use
`docker compose up --build`, which starts one for you.

```bash
pip install -r requirements.txt
python3 main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

Give it something to deliver, and watch it land:

```bash
mosquitto_pub -h localhost -p 1883 -t 'ecv1/gw-01/sim/main/data/temperature-1' \
  -m '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"}}}'
ls -R ./out            # archive/temperature-1/<uuid>.json
```

### Building against the unreleased library (local-dev only)

`requirements.txt` names the `edgecommons` library in the form you chose with `--dep-source`. To
build against a sibling monorepo checkout instead:

```bash
pip install -e ../core/libs/python
```

## Run under Greengrass

```bash
python3 main.py --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

Packaged with the **GDK** using `gdk-config.json` and `recipe.yaml`. The recipe's default
configuration ships one working sink, delivering into the component's work dir (writable by the
non-root `ggc_user`) — because a sink with no instances has nothing to deliver and refuses to start.

```bash
gdk component build
gdk component publish
```

## Deploy to Kubernetes

The Kubernetes artifacts (`Dockerfile`, `k8s/`) exist only when this component was scaffolded with
**KUBERNETES** as a target platform.

```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest   # or: kind load docker-image ...
# set `image:` in k8s/deployment.yaml (replace REPLACE_ME), then:
kubectl apply -f k8s/
```

The scaffold's sink delivers into an **emptyDir** at `/data/out`, which dies with the pod. That is
fine for a first run and wrong for a sink you rely on: point it at a PersistentVolumeClaim, or at a
destination that is not this pod's disk, before you trust it with data.

With `--platform auto` the library detects KUBERNETES from the ServiceAccount token, reads its config
from the mounted ConfigMap (`CONFIGMAP` source, hot-reloaded on `kubectl apply`), uses the MQTT
transport from that same ConfigMap, and resolves identity from the Downward API — so the Deployment
needs no command-line args.

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`).
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS).
- `-t/--thing <name>` — IoT Thing name.

## UNS identity & topics

Topics live in the unified namespace (`ecv1/{device}/{component}/{instance}/{class}/…`). The
component's place in it comes from the top-level `hierarchy` + `identity` config blocks (see
`test-configs/config.json`; the last hierarchy level is always the resolved thing name). A sink's
inbound filter is named by config; everything it publishes (`evt`, and the library's `state` /
`metric`) is minted through `gg.uns()` — never hand-written. The reserved classes
(`state`/`metric`/`cfg`/`log`) are library-owned and rejected on direct publish.
