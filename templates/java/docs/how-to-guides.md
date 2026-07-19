# How-to Guides

> This documents the generated scaffold; rewrite it as you build the component out.

Recipes for specific tasks. Each assumes you already have the scaffold building and running (see the
[tutorial](tutorial.md)). For the concepts behind these steps, see [explanation.md](explanation.md).

---

## Replace the demo surface with your own business logic

**Goal:** keep the wiring (config, messaging, metrics, lifecycle) and replace the four demonstrated
pieces in `<<COMPONENTNAME>>.java` with your own.

1. **Metric.** Replace the `loopTicks` definition/emit with your own `MetricBuilder.create(name)…`
   call — pick a name and measures that mean something for your component, and emit them on
   whatever cadence makes sense (not necessarily the same loop tick).
2. **Data signal.** Replace the `demo-signal` publish with your own `data().signal(id)…addSample(…)`
   calls — one per real reading your component produces, not a synthetic sine wave.
3. **Event.** Replace `sample-event` with events tied to actual occurrences (a threshold crossed, a
   connection lost/restored) rather than a fixed timer; use `raiseAlarm`/`clearAlarm` for a stateful
   alarm instead of one-shot `emit`.
4. **Command verb.** Replace `set-greeting` with your own verb(s), registered the same way via
   `EdgeCommonsBuilder.configureCommands(...)` before `.build()` — install every custom verb before
   the command-inbox subscription can become `ACTIVE`.

None of this is required by the library — a bare scaffold with none of these still runs — it exists
so the demonstrated surface is live end-to-end out of the box.

---

## Report per-instance connectivity

**Goal:** if your component owns a connection (a database pool, an upstream API, a device session),
report it the way an adapter reports device connectivity.

Replace `instanceConnectivity()`'s empty list with one `InstanceConnectivity` entry per connection —
`connected` is the one normalized field every console can render a health dot from; `state` is your
own vocabulary for what a boolean can't say; `attributes` is an open bag for domain data. This is a
**cached status read**, never live I/O — it runs on the heartbeat thread every tick.

---

## Deploy to a platform

**Goal:** run the component on HOST, Greengrass, or Kubernetes.

**HOST (Docker / bare host):**
```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./messaging.json \
  -c FILE ./config.json -t my-thing
```

**Greengrass:**
```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform GREENGRASS -t my-thing
# package: gdk component build && gdk component publish
```

**Kubernetes:** the `Dockerfile` and `k8s/` manifests are emitted when KUBERNETES is a selected
target platform. Edit `k8s/configmap.yaml` and re-`kubectl apply -f k8s/` to hot-reload config in
place (mount the whole ConfigMap volume — never `subPath` — or the hot-reload watch breaks).

---

## Build against the unreleased library (local development)

**Goal:** build this component against a sibling `libs/java` checkout instead of GitHub Packages.

```bash
cd ../core/libs/java && mvn install -DskipTests
mvn -Dedgecommons.version=<the version that install just printed> package
```

See the `edgecommons.version` property comment in `pom.xml`.
