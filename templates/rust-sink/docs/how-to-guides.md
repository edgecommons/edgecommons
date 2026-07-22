# How-to Guides

*This documents the generated scaffold; rewrite it as you build the component out.*

Recipes for specific tasks. Each assumes the sink builds and runs (see the [tutorial](tutorial.md)).
For concepts see [explanation.md](explanation.md); for exhaustive options see [reference/](reference/).

---

## Add a real destination backend

Everything lives behind one trait in `src/dest.rs` — **this is the whole seam**:

```rust
#[async_trait]
pub trait Destination: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn deliver(&self, item: &Item) -> Result<Delivered>;
    async fn verify(&self, item: &Item, delivered: &Delivered) -> Result<()>;
}
```

1. Add a variant to `DestinationConfig` (`{"type": "s3", ...}`, matching a new `oneOf` branch in
   `config.schema.json`'s `$defs.destination`).
2. Implement `Destination` for your backend: `deliver` must land the item at a **stable,
   deterministic key** (so a redelivery overwrites, never duplicates) and must not return `Ok` until
   the data is *live* — not staged, not pending. `verify` must independently confirm what landed
   matches what was sent, **before** the caller releases the source.
3. Wire it into `build()`, matching on the new `DestinationConfig` variant.
4. Classify your backend's failure modes: `DeliverError::Transient` (a timeout, a 503, a full disk
   someone will empty — worth retrying) vs. `Permanent` (bad credentials, a malformed key, a missing
   bucket — retrying wastes the budget and floods the log).

**The pattern `LocalDestination` demonstrates and any real backend should keep:** write to a
temporary location and atomically move/rename into the final key, so a reader never observes a
half-written object and a crash mid-write leaves no corrupt artifact at the real key. Object stores
typically give you this for free (a single `PutObject`/`Upload` call either lands or it doesn't); a
filesystem or anything without atomic writes needs the temp-file-then-rename dance explicitly.

---

## Tune retry

| You want… | Set |
|-----------|-----|
| Faster/slower first retry | `retry.baseDelayMs` (doubles each attempt) |
| A lower/higher backoff ceiling | `retry.maxDelayMs` |
| To give up sooner/later | `retry.giveUpAfterMs` — a **time budget**, not an attempt count |

`giveUpAfterMs` is deliberately a duration, not a count: "twenty attempts" means something very
different at a 1-second backoff than at a 15-minute one, while "keep trying for an hour" means the
same thing at every cadence — the number an operator can actually reason about.

---

## Add another sink

Each entry of `component.instances[]` is one sink — independent, one task each:

```jsonc
{
  "id": "alerts",
  "subscribe": "ecv1/+/+/+/evt/critical/#",
  "destination": { "type": "local", "path": "./alerts" },
  "retry": { "baseDelayMs": 500, "giveUpAfterMs": 600000 }
}
```

A sink's destination **is** its instance — the `state` keepalive and `sb/status`-equivalent surface
report one connectivity entry per configured sink, moved by the same delivery ladder that emits its
events.

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
ServiceAccount token — config from the mounted ConfigMap, identity from the Downward API. If your
real destination is a directory (as the local one is), remember a container's filesystem is
ephemeral unless you mount a volume at that path.

---

## Wire CI

`.github/workflows/ci.yml` calls the org's reusable `component-ci.yml` (build/test/clippy) plus an
in-repo `coverage` job (`cargo llvm-cov --fail-under-lines 90`). Push the generated repo to GitHub
and add the `EDGECOMMONS_READ_TOKEN` secret if your dependency form needs a private git fetch, plus
whatever credentials your real destination backend's tests need (as a repo secret, never committed).

Commit `Cargo.lock` after your first build — the template ships without one (generating one needs
the toolchain and, for a `registry`/`pinned-rev` dependency, network access, which the scaffold
itself never uses at generation time). `edgecommons component validate` warns if it is missing.

`.github/workflows/deploy-docs.yml` is a no-op until the repo carries the
`CLOUDFLARE_DEPLOY_HOOK` secret and is registered in `registry/components.json`.
