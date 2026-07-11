# <<COMPONENTNAME>>

A **processing component** (`<<COMPONENTFULLNAME>>`) written in Java on top of the `edgecommons`
Java library, generated from the EdgeCommons Java **processor** template by the `edgecommons` CLI.
The library gives you the standard CLI contract, configuration, logging, messaging, metrics,
heartbeat, the command inbox and the UNS; this template gives you the *processor archetype* —
subscribe, transform, forward — so you write only the transformation.

```text
  subscribe(filter) ──► bounded queue ──► one worker thread per route ──► publish
                                             (Pipeline)                  local | northbound
```

## Project layout

| Path | Purpose |
|------|---------|
| `src/main/java/…/<<COMPONENTNAME>>.java` | Entry point: builds the runtime, parses routes, subscribes, dispatches. Holds the **self-echo guard** and the **identity restamp**. |
| `src/main/java/…/Processor.java` | **The interface you implement.** One stage: one message in, *zero or more* out, plus an `onTick` hook. |
| `src/main/java/…/Pipeline.java` | An ordered chain of stages — the output of each is the input of the next. |
| `src/main/java/…/Stages.java` | The two demo stages: `FieldEquals` (a filter) and `CountPerTick` (a stateful rollup). Replace them. |
| `src/main/java/…/RouteConfig.java` | Strict parsing of one `component.instances[]` entry into a route. |
| `src/main/java/…/ProcMsg.java` | The unit that flows through a pipeline: a message plus the topic it arrived on. |
| `config.schema.json` | What this component's own config accepts (`component.global` + each instance). |
| `test-configs/config.json` | A working example config — one route, two stages. |
| `recipe.yaml`, `gdk-config.json` | Greengrass component recipe + GDK config. |
| `compose.yaml`, `supervisor/` | HOST platform: a throw-away broker + the component, and a supervisord program block. |
| `Dockerfile`, `k8s/` | Container image + Kubernetes manifests. |

## The archetype

### One route per instance

Each entry of `component.instances[]` is **one route**: topic filters, a pipeline of stages, a
publish topic and a target. Routes are independent — one worker thread each — so a slow route
cannot stall another, and the per-key state inside a stage needs no lock (a stage is *not* required
to be thread-safe).

### A stage returns 0..N messages

A filter drops (returns nothing), a projection maps (returns one), an aggregator fans out (returns
several). `0..N` covers all three without a special case — and it is what lets `onTick(nowMs)`
exist: a **stateful** stage accumulates in `process` and emits in `onTick`, so time-driven output is
not a different mechanism from data-driven output. A tick flows through the *rest* of the pipeline
on the same pass, so a window closing in stage 1 is still projected by stage 2 immediately rather
than waiting for the next message to shake it loose.

### Why a processor uses `getMessaging()` and not `getData()`

This is the mistake the archetype invites. The `data()` facade is for a component that *produces*
readings: it mints its own topic from a signal id and imposes the `SouthboundSignalUpdate` body. A
processor is **payload-agnostic** — it republishes what it was handed, on a topic its route names.
Routing that through `data()` would rewrite both the topic and the body, which is exactly what a
republisher must not do. So: raw `edgeCommons.getMessaging()`, and topics from config.

### The self-echo guard

A processor that publishes onto a class it also subscribes to will consume its own output,
reprocess it, republish it, and saturate the device. `isSelfEcho(...)` drops any message carrying
our own device + component identity. **Do not remove it** because "my route does not do that today"
— a topic filter is config, and config changes.

### The identity restamp

What we publish is **ours**, not the producer's. Every dispatched message is rebuilt with
`.withConfig(configManager)`, which stamps the envelope's `identity` block. Without the restamp the
fleet cannot tell who emitted a message — and the self-echo guard downstream cannot work either.

### The queue is bounded, and a drop is counted

Each route's queue is an `ArrayBlockingQueue(maxQueue)` and the subscription handler `offer()`s
into it — never `put()`s. A full queue **drops and counts**; it does not block the transport's
dispatch thread. The `dropped` measure of the `processorThroughput` metric is what makes that
visible: a processor that silently discards messages is worse than one that crashes.

## Configure

`component.global` carries `defaults` (`tickMs`, `maxQueue`); each `component.instances[]` entry is
a route. `config.schema.json` is the contract, and `RouteConfig.parse` enforces it at runtime —
including rejecting an **unknown key**, because a typo'd route key is a mistake, not a no-op.

```json
{
  "id": "rollup",
  "subscribe": ["ecv1/+/+/+/data/#"],
  "publishTopic": "ecv1/gw-01/<<COMPONENTNAME>>/rollup/data/summary",
  "target": "local",
  "pipeline": [
    { "fieldEquals": { "path": "signal.id", "value": "temperature-1" } },
    { "countPerTick": {} }
  ],
  "tickMs": 10000
}
```

## Add a stage

1. Implement `Processor` in `Stages.java` — `process` for arrival-driven output, `onTick` for
   time-driven output.
2. Add a `case` to `RouteConfig.buildStage`.
3. Add a branch to `config.schema.json`'s `$defs/stage`.

## Build & test

```bash
mvn package        # the shaded jar: target/<<JARNAME>>-1.0.0.jar
mvn test           # the archetype's guard rails: 0..N stages, onTick, the self-echo guard
```

## Run locally (HOST platform, MQTT transport)

Start a local MQTT broker, then:

```bash
java -jar target/<<JARNAME>>-1.0.0.jar \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

Or containerized, with a throw-away broker on the compose network:

```bash
docker compose up --build
```

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT | CONFIGMAP`
  (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## UNS identity & topics

Topics live in the unified namespace (`ecv1/{device}/{component}/{instance}/{class}/{channel…}`).
The component's own identity is config-driven: the top-level `hierarchy`
(`{"levels": ["site", "device"]}`) + `identity` (`{"site": "factory-1"}`) blocks, the last hierarchy
level's value being the resolved thing name (`-t`). A processor's **publish** topics come from its
route config, not from `getUns()` — that is what makes it a republisher — but the reserved classes
(`state` / `metric` / `cfg` / `log`) are library-owned and are rejected on direct publish, so a
route's `publishTopic` must name an application class (`data`, `evt`, `app`, `cmd`).

## Deploy to Greengrass

```bash
gdk component build
gdk component publish
```

## Deploy to Kubernetes

```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
# set image: in k8s/deployment.yaml (replace REPLACE_ME), then:
kubectl apply -f k8s/
```

The ConfigMap is mounted as a **directory** at `/etc/edgecommons`; edit `k8s/configmap.yaml` and
`kubectl apply -f k8s/` again to hot-reload the component config in-process (no restart).

## The edgecommons dependency

`pom.xml` resolves `com.mbreissi.edgecommons:edgecommons` by version from GitHub Packages. For local
development against a sibling checkout, `mvn install` the library (`core/libs/java`) into your `~/.m2`
and point the pom at whatever version that installs as:

```bash
mvn -Dedgecommons.version=<sibling-version> package
```
