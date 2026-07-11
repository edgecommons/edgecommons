# <<COMPONENTNAME>>

A **sink component** (`<<COMPONENTFULLNAME>>`) written in Java on top of the `edgecommons` Java
library, generated from the EdgeCommons Java **sink** template by the `edgecommons` CLI. The library
gives you the standard CLI contract, configuration, logging, messaging, metrics, heartbeat, the
command inbox and the UNS; this template gives you the *sink archetype* — the discipline that keeps
data from being lost on its way out.

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

A sink is the last thing standing between data and its destination. It consumes work, delivers it
outward, and **only then** lets go of the source.

## Project layout

| Path | Purpose |
|------|---------|
| `src/main/java/…/<<COMPONENTNAME>>.java` | Entry point: builds the runtime, parses sinks, consumes, and runs the deliver → verify → retry → report loop. |
| `src/main/java/…/Destination.java` | **The interface you implement.** `kind()` / `deliver(item)` / `verify(item, delivered)`, plus the `build(...)` factory. |
| `src/main/java/…/LocalDestination.java` | The reference backend: temp file + **atomic rename**, and a verify that actually checks. |
| `src/main/java/…/DeliverException.java` | The failure classification: **transient vs permanent**. |
| `src/main/java/…/SinkConfig.java` | One `component.instances[]` entry, and the retry policy. |
| `src/main/java/…/Item.java`, `Delivered.java` | The unit of work (payload + **stable key**) and the proof of what landed. |
| `config.schema.json` | What this component's own config accepts (`component.global` + each instance). |
| `test-configs/config.json` | A working example config — one sink to a local directory. |
| `recipe.yaml`, `gdk-config.json` | Greengrass component recipe + GDK config. |
| `compose.yaml`, `supervisor/` | HOST platform: a throw-away broker + the component, and a supervisord program block. |
| `Dockerfile`, `k8s/` | Container image + Kubernetes manifests. |

## The archetype

The **ordering** is the archetype, and every step earns its place.

### 1. Deliver idempotently, to a stable key

The same item always lands at the same place, so a redelivery **overwrites** rather than
duplicating. This is what makes retry safe: *a sink that cannot retry without duplicating cannot
retry at all.* `keyFor(...)` derives the key from the sink id, the topic leaf and the message's
envelope uuid — never from the clock, a counter, or a fresh random id.

`LocalDestination` shows what a backend owes you here: it writes to a temp file and **renames it
into place**. A rename within a filesystem is atomic, so a reader never observes a half-written
object, and a crash mid-write leaves no corrupt artifact at the real key.

### 2. Verify before you confirm

`deliver` returning without throwing is not evidence. Releasing the source on that basis — without
checking what actually landed — is how you end up having deleted the only copy. `verify(item,
delivered)` runs **before** the confirmation, and a mismatch is a failure, not a warning.

### 3. Classify the failure

`DeliverException` is transient or permanent, and getting it wrong is expensive in both directions:
retrying a permanent error burns the budget and floods the log, while giving up on a transient one
loses data a second attempt would have delivered. When genuinely unsure, prefer **transient** — a
wrongly-transient failure wastes retries; a wrongly-permanent one loses data.

### 4. Retry with full jitter, against a time budget

Exponential backoff, capped — and the delay is drawn uniformly from `[0, window)`, not fixed at the
window's edge. The jitter is not decoration: without it, every component that lost the same endpoint
retries at the *same instant*, and an endpoint that is already struggling gets a synchronized
thundering herd on every backoff boundary.

The give-up is a **time budget** (`giveUpAfterMs`), not an attempt count. "Twenty attempts" means
something different at 1 s and at 15 min of backoff; "keep trying for an hour" means the same thing
at every cadence, and it is what an operator can actually reason about.

### 5. Report every transition

A sink that fails quietly is indistinguishable from one that is idle. The event ladder goes out on
the UNS `evt` class:

| Event | Severity | When |
|---|---|---|
| `delivery-started` | Info | An item entered the loop. |
| `delivery-completed` | Info | Delivered **and verified**; carries `attempts` and `elapsedMs`. |
| `delivery-failed` | Warning | A transient failure; carries `willRetry` and `nextAttemptInMs`. |
| `delivery-exhausted` | **Critical** | Permanent failure, or the time budget is spent. **This is data that did not arrive.** |

Subscribe `ecv1/+/+/+/evt/critical/#` to watch only the ones that mean something was lost. The
`sinkDeliveries` metric carries the same story as counters: `received`, `delivered`, `retried`,
`exhausted`, `dropped`.

### The queue is bounded, and a drop is counted

Each sink's queue is an `ArrayBlockingQueue(maxQueue)` and the subscription handler `offer()`s into
it — never `put()`s. A full queue **drops and counts**; it does not block the transport's dispatch
thread.

## Where the work comes from

This scaffold's source is a **subscription**: it consumes messages off the bus and delivers each
one. That is the common case. If your source is a watched directory or a polled API, replace the
`subscribe` call in `run()` — everything downstream of `deliverWithRetry` is unchanged, which is the
point of the seam.

## Configure

`component.global` carries `defaults` (`retry`, `maxQueue`); each `component.instances[]` entry is a
sink. `config.schema.json` is the contract, and `SinkConfig.parse` enforces it at runtime —
including rejecting an **unknown key**, because a typo in a sink's config is how data quietly goes
to the wrong place.

```json
{
  "id": "archive",
  "subscribe": "ecv1/+/+/+/data/#",
  "destination": { "type": "local", "path": "./out" },
  "retry": { "baseDelayMs": 1000, "maxDelayMs": 900000, "giveUpAfterMs": 3600000 },
  "maxQueue": 256
}
```

## Add a destination

1. Implement `Destination` — `kind()`, `deliver(item)` (idempotent, to `item.key()`), and a
   `verify(item, delivered)` that actually checks what landed.
2. Classify every failure you throw as transient or permanent.
3. Add a `case` to `Destination.build`.
4. Add a branch to `config.schema.json`'s `$defs/destination`.

## Build & test

```bash
mvn package        # the shaded jar: target/<<JARNAME>>-1.0.0.jar
mvn test           # the archetype's guard rails: backoff/jitter/budget, idempotent redelivery, verify
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
The component's identity is config-driven: the top-level `hierarchy`
(`{"levels": ["site", "device"]}`) + `identity` (`{"site": "factory-1"}`) blocks, the last hierarchy
level's value being the resolved thing name (`-t`). A sink *consumes* topics its config names and
*emits* its event ladder through `getEvents()`, which mints `evt/{severity}/{type}` from the body's
own severity and type — so the topic and the body can never disagree.

## Deploy to Greengrass

```bash
gdk component build
gdk component publish
```

The recipe's default destination is under the component's Greengrass **work** directory, which is
writable by `ggc_user` and is not replaced by a deployment. Point it elsewhere and you must grant
that user write access to the new path.

## Deploy to Kubernetes

```bash
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
# set image: in k8s/deployment.yaml (replace REPLACE_ME), then:
kubectl apply -f k8s/
```

The pod runs with a **read-only root filesystem**, so a `local` destination needs a writable volume
mounted at its path (an `emptyDir` is enough for a scratch demo; a PersistentVolumeClaim is what you
want if the data must outlive the pod).

## The edgecommons dependency

`pom.xml` resolves `com.mbreissi.edgecommons:edgecommons` by version from GitHub Packages. For local
development against a sibling checkout, `mvn install` the library (`core/libs/java`) into your `~/.m2`
and point the pom at whatever version that installs as:

```bash
mvn -Dedgecommons.version=<sibling-version> package
```
