# How-to Guides

*This documents the generated scaffold; rewrite it as you build the component out.*

Recipes for specific tasks. Each assumes the component builds and runs (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Replace the demo metric

`src/app.rs::App::new` defines `loopTicks` (`tickCount` counter + `uptimeSecs` gauge-like measure)
via `MetricBuilder`. Define your own metric the same way — `MetricBuilder::create(name)
.with_config(&gg.config()).add_measure(...).add_dimension(...).build()` — then call
`self.metrics.emit_metric(name, values)` wherever your own event happens (not necessarily on a fixed
timer). Keep dimensions low-cardinality.

---

## Replace the demo data signal

`gg.data().publish_value(DATA_SIGNAL_ID, value).await?` is the one-line path for a scalar reading
with an implicit `GOOD` quality. When your source knows a read failed or is stale, use the fuller
builder instead: `gg.data().signal(id).name(...).sample(Sample::with_quality(value,
Quality::Bad)).build()` then `.publish()`. A component that only ever calls `publish_value` and
never reports `Bad`/`Uncertain` is implicitly claiming every read always succeeds — true for this
demo's sine wave, rarely true for a real source.

---

## Replace the demo event

`gg.events().emit(severity, type, message, context).await?` derives the `evt/{severity}/{type}`
topic from the arguments, so the topic and body can never disagree. Emit on **actual occurrences** —
a threshold crossed, a connection lost/restored — not on a fixed timer as this demo does. Use
`raise_alarm`/`clear_alarm` instead of `emit` for a **stateful** condition (something that starts and
later ends), so a fleet consumer sees both the raise and the clear on the same channel.

---

## Add your own command verb

Register additional verbs with `gg.commands().register(name, command_handler(...))`, or install them
before the inbox goes active via `EdgeCommonsBuilder::configure_commands` (as `set-greeting` does in
`src/main.rs`/`src/app.rs`, so state exists before the first request can arrive). Return
`Err(CommandError::new(CODE, message))` for a bad request rather than panicking — a malformed
command should never take the component down.

---

## Report a real connection

This scaffold's instance-connectivity provider (`gg.set_instance_connectivity_provider`) currently
returns an empty `Vec` — a real answer, since the scaffold owns no southbound connections. The moment
this component gains one (a device, a database, an upstream API), return one
`InstanceConnectivity::of(&id, connected)` per connection, `.with_state(...)` in your own vocabulary,
`.with_attributes(...)` for domain data. The same sample then feeds both the `state` keepalive's
`instances[]` and the built-in `status` command verb — see the comment in `App::new` for the shape,
and consider promoting this scaffold to the `protocol-adapter`/`sink`/`processor` archetype template
if a connection becomes the component's central concern.

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
