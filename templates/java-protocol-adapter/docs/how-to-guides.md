# How-to Guides

> This documents the generated scaffold; rewrite it as you build the component out.

Recipes for specific tasks. Each assumes you already have the scaffold building and running (see the
[tutorial](tutorial.md)). For the concepts behind these steps, see [explanation.md](explanation.md);
for exhaustive option lists, see [reference/](reference/).

---

## Implement a real device backend

**Goal:** replace the simulator with your protocol, without touching anything above the seam.

1. Add your protocol's client SDK to `pom.xml` where the `TODO(adapter)` comment is.
2. In `Device.java`, add a case to `backendFor(String adapter)` for your protocol's name, and
   implement `DeviceBackend`:
   - `connect(ConnectionConfig)` — open a session; throw `DeviceException.transientError(...)` for a
     retryable failure (link down, timeout) and `DeviceException.permanent(...)` for one that will
     never succeed (bad config, rejected credentials) — the supervisor backs off differently for each.
   - `inventory(ConnectionConfig)` — the signal list `sb/signals` returns, from config alone, with
     **no device round-trip**.
3. Implement `DeviceSession`:
   - `readSignals()` — read every configured signal. A single bad register should come back as one
     `Reading` with `Quality.BAD`, not an exception — throw only when the **connection** itself is
     broken.
   - `writeSignal(id, value)` — write one value; throw on rejection or a dead link.
   - `browse(cursor, max)` — override only if your protocol has discovery; the default answers
     `BROWSE_UNSUPPORTED`, which is the honest answer for a fixed register map.
4. Map your protocol's native status/quality to `Quality.GOOD | BAD | UNCERTAIN`, keeping the native
   code in `qualityRaw` for diagnostics.
5. Add your device's real connection keys to `config.schema.json`'s `$defs/device.properties.connection`
   (it is `additionalProperties: true` by design — every protocol needs different keys there).

Nothing in `<<COMPONENTNAME>>.java`, `Commands.java`, or `Metrics.java` needs to change — that is the
point of the seam.

---

## Add your protocol's metric families

**Goal:** measure the parts of your protocol the generic families don't cover (an inventory, a
poll/subscribe path, a publish path).

`Metrics.java` ships `southbound_health` (mandatory, exact measure set) plus two worked operational
families, `<<COMPONENTNAME>>Connection` and `<<COMPONENTNAME>>Command`. Add
`<<COMPONENTNAME>>Inventory` / `<<COMPONENTNAME>>Poll` / `<<COMPONENTNAME>>Publish` next to them in
`familyDefs()` (register the family, its dimensions, and its measures), pre-define it in
`DeviceMetrics.defineAll()`, and record into it from the worker loop. Follow the total/interval
counter-pair convention already used by the two worked families, and keep dimensions low-cardinality
(`instance`, closed enum values — never a signal id, an address, or raw error text).

---

## Read and write signals from a client

**Goal:** exercise `sb/read`/`sb/write` from your own client, not `mosquitto_pub`.

Both are request/reply on `ecv1/{device}/<<BINNAME>>/cmd/sb/{verb}`. With an EdgeCommons client, use
its `request()` API, which sets `header.name`, `reply_to`, and `correlation_id` for you. See
[Reference — Messaging Interface](reference/messaging-interface.md#the-command-surface) for the exact
request/reply shapes.

---

## Observe health and status

**Goal:** know whether a configured device is connected and healthy.

- **`southbound_health`** — `connectionState`, `publishLatencyMs`, `pollLatencyMs`, `readErrors`,
  `staleSignals`, `reconnects`, dimensioned by `instance`. Routes to wherever
  `metricEmission.target` sends it.
- **State keepalive** — subscribe `ecv1/+/+/+/state`; the RUNNING keepalive's `instances[]` carries
  one `{instance, connected, detail}` entry per configured device.
- **`sb/status`** — a pull for one device's `connected`/`state`/`paused`/`endpoint` plus its
  connection counters — the same data the keepalive pushes, on demand.
- **Events** — `device-connected` / `device-unreachable` (raise/clear pair) ride `evt/info/…` and
  `evt/critical/…`; subscribe `ecv1/+/+/+/evt/#`.

---

## Pause and resume telemetry without disconnecting

**Goal:** stop a device from publishing without dropping its session.

`sb/pause` / `sb/resume` toggle a per-device flag the worker checks before every poll; the session
stays connected the whole time, so resuming does not pay a reconnect. `sb/status`/the connectivity
provider report `state: "PAUSED"` while paused and still-connected, so an operator can tell "paused"
apart from "down."

---

## Deploy to a platform

**Goal:** run the adapter on HOST, Greengrass, or Kubernetes.

**HOST (Docker / bare host):**
```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./messaging.json \
  -c FILE ./config.json -t my-thing
```

**Greengrass (on-device):** config comes from the deployment; transport is IPC.
```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform GREENGRASS -t my-thing
# package: gdk component build && gdk component publish
```

**Kubernetes:** the `Dockerfile` and `k8s/` manifests are emitted when KUBERNETES is a selected
target platform; config comes from a mounted ConfigMap, identity from the Downward API.

---

## Validate against a real simulator or device

**Goal:** run the gated live-integration test instead of the unit suite.

`src/test/java/.../LiveSimIT.java` self-skips unless `EC_LIVE_SIM` is set. Point it at a running
simulator or device and run:

```bash
EC_LIVE_SIM=sim://device-1 mvn test -Dtest=LiveSimIT
```

It connects, runs one poll cycle, and asserts on readings and quality — adapt the assertions once
`Device.java` talks to your real protocol.
