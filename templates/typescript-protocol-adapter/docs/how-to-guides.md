This documents the generated scaffold; rewrite it as you build the component out.

# How-to Guides

Recipes for specific tasks. Each assumes the component builds and runs (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Replace the simulator with a real protocol

`src/device.ts` is the seam. Implement `DeviceSession` (or extend `BaseDeviceSession` and override
just `readSignals`/`writeSignal`) for your protocol, register it in `backendFor`, and set
`adapter` in config to its `kind`. Nothing in `src/app.ts` or `src/commands.ts` needs to change —
they are written against the interface, never against `SimBackend`.

```ts
export class MyProtocolSession extends BaseDeviceSession {
  async readSignals(): Promise<Reading[]> { /* your protocol read */ }
  async writeSignal(signalId: string, value: unknown): Promise<void> { /* your protocol write */ }
  // override readNamed/browse only if your protocol can do better than "read-all-and-filter" /
  // "no discovery" — the base class's defaults are honest without them.
}
```

Keep the boundary rule from the module header: `src/device.ts` must import nothing from
`@edgecommons/edgecommons`. If your session needs the UNS, a topic, or a metric, that need belongs
in `src/app.ts`, not the seam.

---

## Add a second device

Add another entry to `component.instances[]` — each gets its own connection loop, control channel,
and metrics emitter, so one device being down never disturbs another:

```jsonc
"instances": [
  { "id": "device-1", "adapter": "sim", "connection": { "endpoint": "sim://device-1" }, "pollIntervalMs": 5000, "writes": { "allow": [] } },
  { "id": "device-2", "adapter": "sim", "connection": { "endpoint": "sim://device-2" }, "pollIntervalMs": 2000, "writes": { "allow": [] } }
]
```

With two or more devices, every `sb/*` command needs a body `instance` field naming which one
(`BAD_ARGS` if it's missing, `NO_SUCH_INSTANCE` if it names an unconfigured one).

---

## Allow writes to a signal

Writes are allow-listed **per instance, per signal id** — the allow-list is checked before any
device I/O, so a signal never reaches your `writeSignal` unless it's explicitly permitted:

```jsonc
"writes": { "allow": ["temperature-1", "setpoint-a"] }
```

An empty (or omitted) `allow` makes the instance read-only, which is the correct default for
anything touching a control system.

---

## Read and write signals from a client

Both go through the command inbox (`ecv1/{device}/{component}/cmd/{verb}`). Set `header.name` to
the verb and `header.reply_to` + `header.correlation_id` for the reply.

```
publish ecv1/<device>/<<COMPONENTNAME>>/cmd/sb/read
  {"header":{"name":"sb/read","reply_to":"app/r","correlation_id":"1"},
   "body":{"signals":[{"name":"temperature-1"}]}}
subscribe app/r → {"ok":true,"result":{"reads":[...]}}

publish ecv1/<device>/<<COMPONENTNAME>>/cmd/sb/write
  {"header":{"name":"sb/write","reply_to":"app/r","correlation_id":"2"},
   "body":{"writes":[{"signalId":"temperature-1","value":42.5}]}}
subscribe app/r → {"ok":true,"result":{"written":1,"results":[...]}}
```

A signal-ref accepts `signalId`, `id`, or `name` (looked up against the configured inventory).
Full payload shapes: [reference/messaging-interface.md](reference/messaging-interface.md).

---

## Pause and resume telemetry

`sb/pause` stops the poll loop (and future polls) without dropping the connection; `sb/resume`
restarts it. Both are idempotent — pausing an already-paused instance replies `{changed: false}`.
Useful during maintenance windows, or before a `sb/write` sequence you don't want a concurrent poll
to interleave with.

```
publish ecv1/<device>/<<COMPONENTNAME>>/cmd/sb/pause   {"header":{"name":"sb/pause","reply_to":"app/r","correlation_id":"3"},"body":{}}
publish ecv1/<device>/<<COMPONENTNAME>>/cmd/sb/resume  {"header":{"name":"sb/resume","reply_to":"app/r","correlation_id":"4"},"body":{}}
```

`repoll` refuses with `BAD_ARGS` while paused ("resume first") — pausing means *nothing* touches
the device, not even an on-demand poll.

---

## Force a reconnect or an immediate poll

```
publish ecv1/<device>/<<COMPONENTNAME>>/cmd/reconnect {"header":{"name":"reconnect","reply_to":"app/r","correlation_id":"5"},"body":{}}
publish ecv1/<device>/<<COMPONENTNAME>>/cmd/repoll    {"header":{"name":"repoll","reply_to":"app/r","correlation_id":"6"},"body":{}}
```

`reconnect` drops the current session and re-establishes it (one bounded attempt, confirmed by
reply). `repoll` triggers one poll cycle right now, independent of `pollIntervalMs`.

---

## Add your protocol's own metric families

`src/metrics.ts` ships `southbound_health` (the canonical set every adapter emits) plus two worked
operational families, `<<COMPONENTNAME>>Connection` and `<<COMPONENTNAME>>Command`. Your protocol
almost certainly wants more: an **inventory** family (what's configured), a **poll** family (read
volume, decode errors, samples good/bad), and a **publish** family (messages published, publish
latency). Add them next to the two worked examples — register each in `familyDefs()` and
pre-define it in `DeviceMetrics.defineAll()`; the record → drain → emit shape is copy-able from
`CmdCounters`. See `modbus-adapter/modbus_adapter/metrics.py` and
`ethernet-ip-adapter/crates/ethernet-ip-adapter/src/metrics.rs` for the full worked set. Keep every
new dimension **low-cardinality** — never a signal name, address, or error string (see
[reference/metrics.md](reference/metrics.md)).

---

## Point the live-sim suite at a real device

`test/live-sim.test.ts` self-skips unless `EC_LIVE_SIM` is set. Point it at whatever your
`connect()` needs (the built-in simulator just needs a non-empty endpoint string; a real protocol
needs a reachable device or simulator, e.g. the permanent `ggcommons-modbus-sim` container or a
protocol-specific simulator like cpppo/OpENer):

```bash
EC_LIVE_SIM=sim://device-1 npm test
```

Update the suite to build your real backend (via `backendFor`) and connect through
`cfg.connection.endpoint = process.env.EC_LIVE_SIM` once you've replaced the simulator — the shape
(connect → one poll cycle → assert readings + quality) is the same either way.

---

## Deploy to a platform

**HOST:** `node dist/main.js --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`

**Greengrass:** package per `gdk-config.json`/`recipe.yaml`; config comes from the deployment
(`--platform GREENGRASS -c GG_CONFIG`).

**Kubernetes:** build the image (`Dockerfile`), apply `k8s/` (config from a mounted ConfigMap,
identity from the Downward API).

---

## Observe health and status

- **Metric** `southbound_health` — `connectionState`, `publishLatencyMs`, `pollLatencyMs`,
  `readErrors`, `staleSignals`, `reconnects` (the exact SOUTHBOUND.md §5 set).
- **Operational metrics** `<<COMPONENTNAME>>Connection` (connect lifecycle) and
  `<<COMPONENTNAME>>Command` (the `sb/*` surface, dimensioned `instance`×`verb`×`result`).
- **State keepalive:** `ecv1/{device}/{component}/state` every ~5 s, carrying each instance's
  connectivity (`connected`, `state`, `endpoint`) in its `instances[]` array — the same sample
  `sb/status` answers on demand.
- **Events:** `evt/info/device-connected`, `evt/critical/device-unreachable` (a stateful alarm,
  raised on drop and cleared on reconnect), `evt/warning|info/adapter-paused|adapter-resumed`.
- **Status verb:** `sb/status` → `{connected, state, paused, endpoint, metrics}`.
