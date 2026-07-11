# <<COMPONENTNAME>>

A **processing component** (`<<COMPONENTFULLNAME>>`) written in Python on top of the `edgecommons`
Python library, generated from the EdgeCommons Python processor template by the `edgecommons` CLI.
It gives you the library's standard CLI contract, configuration, logging, messaging, metrics and
heartbeat — so you write only the transformation, in [`app/pipeline.py`](app/pipeline.py).

## What a processor is

A processor **subscribes**, **transforms**, and **forwards**.

```text
  subscribe(filter) ──► bounded queue ──► one thread per route ──► publish
                                             (Pipeline)           local | northbound
```

Each entry of `component.instances[]` is **one route**: topic filters, a pipeline of stages, and a
target. Routes are independent — one thread each — so a slow route cannot stall another, and per-key
state inside a stage needs no lock.

| Path | What it is |
|------|-----------|
| `main.py` | Entry point — builds `EdgeCommons` and starts the app. |
| `app/<<COMPONENTNAME>>.py` | The wiring: routes, subscriptions, the bounded queue, the tick, publishing, metrics, events. |
| `app/pipeline.py` | **Where your code goes** — the stages, the pipeline, the self-echo guard, the route parser. Pure logic: it does not import the library, so it is unit-testable on its own. |
| `tests/` | `pytest` tests for the invariants below. `python -m pytest` — no broker needed. |
| `config.schema.json` | The config this component itself understands (`component.global` + each `component.instances[]` entry). |
| `test-configs/` | A working `config.json` + the MQTT `standalone-messaging.json` for local HOST runs. |

## The pipeline

A stage takes one message and returns **zero or more** messages, so one abstraction covers all three
useful shapes: a filter returns nothing, a map returns one, an aggregator fans out. That is also what
lets `on_tick` exist — a *stateful* stage (a window, a debounce, a batch) accumulates in `process`
and emits in `on_tick`, so time-driven output is not a different mechanism from data-driven output. A
tick flows through the rest of the pipeline on the same pass, so a window closing in stage 1 is still
projected by stage 2 without waiting for the next message to shake it loose.

Two demo stages ship: `fieldEquals` (a filter) and `countPerTick` (a stateful rollup). Add your own
to `app/pipeline.py`'s stage table **and** to `config.schema.json`'s `stage` definition — the two are
one contract, and an unknown or misspelt stage is rejected when the route is parsed, not on the first
message.

## The invariants — do not remove these

* **The self-echo guard.** A processor that publishes onto a class it also subscribes to will consume
  its own output, reprocess it, republish it, and saturate the device. `is_self_echo` drops any
  message carrying our own device + component identity. `main.py` also asks the transport not to echo
  (`receive_own_messages(False)`), but only Greengrass IPC can honour that — an MQTT broker
  redelivers our own publishes to our own wildcard subscription like anyone else's. The guard is what
  actually holds.
* **The identity restamp.** What we publish is *ours*, not the producer's. Every outbound message is
  rebuilt through `gg.instance(route.id).new_message(...)`, which stamps this component's
  config-resolved identity with the route's instance token. Without it the fleet cannot tell who
  emitted a message — and the self-echo guard downstream cannot work either.
* **The queue is bounded, and a full queue drops and *counts*.** An unbounded queue does not remove
  backpressure; it relocates the failure to the heap, and by the time you notice you have lost the
  ability to report it. Drops are published as the `dropped` measure of the `processorThroughput`
  metric — a processor that silently discards messages is worse than one that crashes.

### Why a processor uses `get_messaging()` and not `data()`

The mistake this archetype invites. The `data()` facade is for a component that *produces* readings:
it mints its own topic from a signal id and imposes the `SouthboundSignalUpdate` body. A processor is
**payload-agnostic** — it republishes what it was handed, on a topic its route names. Routing that
through `data()` would rewrite both the topic and the body, which is exactly what a republisher must
not do. So: raw `gg.get_messaging()`, and topics from config.

## Configuration

```json
{
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

`target` is `local` (the device-local bus — the common case) or `northbound` (straight out to the
northbound broker). `id` is the route's UNS instance token, so it must be lower-kebab. Unknown keys
are rejected rather than ignored: a config knob that silently does nothing is the worst kind of bug
to find in the field.

## Reported surface

| Surface | Where | Topic |
|---|---|---|
| Metric (`processorThroughput`: `received`, `published`, `dropped`, `errors`) | `gg.get_metrics()` | `ecv1/{device}/{component}/main/metric/processorThroughput` (target-dependent) |
| Event (`publish-failed`) | `gg.instance(route).events()` | `ecv1/{device}/{component}/{route}/evt/warning/publish-failed` |
| `state` keepalive + command inbox (`ping` / `reload-config` / `get-configuration`) | automatic, library-owned | `ecv1/{device}/{component}/main/state`, `…/main/cmd/#` |

## Run locally (HOST platform, MQTT transport)

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`), or use
`docker compose up --build`, which starts one for you.

```bash
pip install -r requirements.txt
python3 main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

Feed the route something to process and watch what it republishes:

```bash
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/data/#' -v
mosquitto_pub -h localhost -p 1883 -t 'ecv1/gw-01/sim/main/data/temperature-1' \
  -m '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.5}]}}'
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
configuration ships one working route — **edit its `publishTopic` so the device token is the thing
name you deploy to** — because a processor with no routes has nothing to run and refuses to start.

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
`test-configs/config.json`; the last hierarchy level is always the resolved thing name). A processor's
`publishTopic` is named by config rather than minted in code — that is the archetype — but everything
the library publishes on your behalf (`state`, `metric`, `evt`) is minted through `gg.uns()`, and the
reserved classes (`state`/`metric`/`cfg`/`log`) are library-owned and rejected on direct publish.
