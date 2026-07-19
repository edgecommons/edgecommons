# How-to Guides

*This documents the generated scaffold; rewrite it as you build the component out.*

Recipes for specific tasks. Each assumes the processor builds and runs (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Write your own stage

A stage implements `Processor` (`src/proc.rs`):

```rust
pub trait Processor: Send {
    fn process(&mut self, m: ProcMsg) -> Out;                  // 0..N messages out
    fn on_tick(&mut self, now_ms: u64) -> Out { Out::new() }    // for stateful stages
}
```

1. Implement the trait for your stage's struct (state lives in `&mut self` — no lock needed, since
   each route's pipeline runs in its own task).
2. Add a matching variant to `StageConfig` (`src/app.rs`) and a case in `StageConfig::build()`.
3. Add the variant's shape to `config.schema.json`'s `$defs.stage` (a single-key object, per the
   existing `fieldEquals`/`countPerTick` pattern).

A **stateless** stage (a filter, a map) only needs `process` — return `Out::new()` to drop a
message, `smallvec::smallvec![m]` to keep/transform it, or several entries to fan out. A **stateful**
stage (a window, a debounce, a batch) accumulates in `process` (typically returning nothing) and
produces its output in `on_tick` — `CountPerTick` is the worked example.

---

## Add another route

Each entry of `component.instances[]` is one route — independent, one task each:

```jsonc
{
  "id": "alarms",
  "subscribe": ["ecv1/+/+/+/evt/critical/#"],
  "publishTopic": "ecv1/gw-01/<<BINNAME>>/alarms/data/summary",
  "target": "northbound",
  "pipeline": [ { "countPerTick": {} } ],
  "tickMs": 5000
}
```

A slow or misbehaving route cannot stall another — they share nothing but the process.

---

## Route to the northbound broker

Set `"target": "northbound"` on a route instead of the default `"local"`. This publishes with
`Qos::AtLeastOnce` over the dual-MQTT provider's northbound session (HOST/Kubernetes) or through the
Nucleus' IoT Core connection (Greengrass) — no `messaging.northbound` config block needed on
Greengrass, since the platform already owns that connection.

---

## Tune queueing and tick cadence

| You want… | Set |
|-----------|-----|
| A stateful stage to emit sooner/later | `pipeline[].tickMs` (per-route) or `component.global.defaults.tickMs` |
| More/less headroom before messages drop | `pipeline[].maxQueue` (per-route) or `component.global.defaults.maxQueue` |

A full queue **drops and counts** (the `dropped` measure of `processorThroughput`) — it never blocks
the subscription's dispatch task. If you are seeing drops, either the route's pipeline is too slow
for its input rate, or `maxQueue` is set too low for a legitimate burst.

---

## Add a metric for your own stage

`src/app.rs`'s `emit_metrics` currently reports the four cross-cutting counters
(`received`/`published`/`dropped`/`errors`) under `processorThroughput`. If your stage needs its own
measure (a per-window average, a distinct error class), add it to that same metric — keep the
dimension set (`instance` only, from the metric's own definition today) low-cardinality, and record
it from inside your stage or from `dispatch`/`run_route`, mirroring the existing `Stats` counters.

---

## Deploy to a platform

**HOST:**
```bash
cargo run -- --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

**Greengrass:** `gdk-config.json` uses the GDK custom build system; `build.sh` compiles with
`--features greengrass` (Linux-only — the SDK is a C-FFI crate needing `libclang`).
```bash
gdk component build
gdk component publish
```
Set a real `publish.bucket` first if `gdk-config.json` still carries the
`edgecommons-set-artifact-bucket` sentinel — `edgecommons component validate` errors on it.

**Kubernetes:** build the image, push or `kind load` it, set `image:` in `k8s/deployment.yaml`, then
`kubectl apply -f k8s/`. With `--platform auto` the library detects Kubernetes from the
ServiceAccount token — config from the mounted ConfigMap, identity from the Downward API.

---

## Wire CI

`.github/workflows/ci.yml` calls the org's reusable `component-ci.yml` (build/test/clippy) plus an
in-repo `coverage` job (`cargo llvm-cov --fail-under-lines 90`). Push the generated repo to GitHub
and add the `EDGECOMMONS_READ_TOKEN` secret if your dependency form needs a private git fetch.

Commit `Cargo.lock` after your first build — the template ships without one (generating one needs
the toolchain and, for a `registry`/`pinned-rev` dependency, network access, which the scaffold
itself never uses at generation time). `edgecommons component validate` warns if it is missing.

`.github/workflows/deploy-docs.yml` is a no-op until the repo carries the
`CLOUDFLARE_DEPLOY_HOOK` secret and is registered in `registry/components.json`.
