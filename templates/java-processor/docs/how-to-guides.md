# How-to Guides

> This documents the generated scaffold; rewrite it as you build the component out.

Recipes for specific tasks. Each assumes you already have the scaffold building and running (see the
[tutorial](tutorial.md)). For the concepts behind these steps, see [explanation.md](explanation.md).

---

## Add a stage

**Goal:** add your own transformation to the pipeline.

1. Implement `Processor` in `Stages.java` (or a new file) — `process(ProcMsg)` returns zero or more
   messages for arrival-driven output; override `onTick(nowMs)` too if your stage is stateful (a
   window, a batch, a debounce) and should emit on a timer instead of (or in addition to) arrival.
2. Add a `case` to `RouteConfig.buildStage` mapping your stage's config key to a `new` instance.
3. Add a matching branch to `config.schema.json`'s `$defs/stage`, with `additionalProperties: false`
   on your stage's argument object — a typo'd stage key should be a startup error, not a silent no-op.

## Add a route

**Goal:** process a second, independent stream of messages.

Add another entry to `component.instances[]` — each route gets its own worker thread, its own bounded
queue, and its own pipeline instance, so routes never interfere with each other's state or backpressure.

## Tune the queue and tick cadence

**Goal:** control how much a route buffers and how often stateful stages flush.

`maxQueue` (default `256`) bounds the per-route queue; when it's full, new messages are **dropped and
counted** (the `dropped` measure), never blocking the transport's dispatch thread. `tickMs` (default
`10000`) is how often `onTick` runs. Both are set per-route or globally under
`component.global.defaults`. Widen `maxQueue` for a bursty source; shorten `tickMs` for a route whose
rollup needs to be fresher.

## Republish to the northbound broker instead of the local bus

**Goal:** send a route's output off-device.

Set `"target": "northbound"` on the route. The processor uses
`messaging.publishNorthbound(topic, msg, Qos.AT_LEAST_ONCE)` for these — everything else about the
route (subscription, pipeline, identity restamp) is unchanged.

## Deploy to a platform

**Goal:** run the component on HOST, Greengrass, or Kubernetes.

**HOST (Docker / bare host):**
```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./messaging.json \
  -c FILE ./config.json -t my-thing
```
Or containerized, with a throw-away broker on the compose network: `docker compose up --build`.

**Greengrass:**
```bash
gdk component build
gdk component publish
```

**Kubernetes:**
```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
kubectl apply -f k8s/
```
The ConfigMap is mounted as a directory; editing it and re-applying hot-reloads route config without
a restart.

## Build against the unreleased library (local development)

```bash
cd ../core/libs/java && mvn install -DskipTests
mvn -Dedgecommons.version=<the version that install just printed> package
```
