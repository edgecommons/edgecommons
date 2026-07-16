# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Rust on top
of the `edgecommons` Rust library, generated from the EdgeCommons Rust component template
by the `edgecommons` CLI. It gives you the library's
standard CLI contract, configuration, logging, messaging, metrics, and heartbeat —
so you write only business logic in [`src/app.rs`](src/app.rs).

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.rs` | Entry point: builds the `edgecommons` runtime from CLI args, runs the app. |
| `src/app.rs` | Your component logic (starts as a minimal app + config-change listener). |
| `Cargo.toml` | Crate manifest. Depends on the `edgecommons` library (path dependency). |
| `recipe.yaml` | Greengrass component recipe (default config + IPC access control). |
| `gdk-config.json` | Greengrass Development Kit config (`build_system: custom` → `build.sh`). |
| `build.sh` | Builds the release binary (with the `greengrass` feature) and stages it for the GDK. |
| `test-configs/` | Sample `config.json` + `standalone-messaging.json` for local runs. |

## Develop & run locally (HOST platform, MQTT transport)

Local development runs on the HOST platform with the MQTT transport (dual-broker MQTT) — no
Greengrass core or Linux/`libclang` toolchain needed. Start a local MQTT broker, then:

```bash
cargo run -- \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

## The demonstrated monitoring + command surface

Beyond the fully-automatic `state` keepalive and command inbox (`ping` / `reload-config` /
`get-configuration`, live with zero code), `src/app.rs` demonstrates the rest of the surface an
edge-console reads/drives (DESIGN-uns §7/§9), through the **app-usable class facades**
(`docs/platform/DESIGN-class-facades.md`) rather than hand-built topics/bodies:

| Surface | Where | Topic |
|---|---|---|
| Metric (`loopTicks`: `tickCount` counter + `uptimeSecs` gauge) | `gg.metrics()` | `ecv1/{device}/{component}/metric/loopTicks` (target-dependent; `messaging` target shown) |
| Data signal (`demo-signal`: a sine-wave reading) | `gg.data().publish_value("demo-signal", value).await?` | `ecv1/{device}/{component}/data/demo-signal` |
| Event (`sample-event`, severity + context) | `gg.events().emit(Severity::Info, "sample-event", message, context).await?` | `ecv1/{device}/{component}/evt/info/sample-event` |
| Custom command verb (`set-greeting`) | `EdgeCommonsBuilder::configure_commands(...)` | `ecv1/{device}/{component}/cmd/set-greeting` |

Subscribe `ecv1/+/+/metric/#`, `ecv1/+/+/data/#` and `ecv1/+/+/evt/#` (add the `ecv1/+/+/+/…` instance-scope form to also catch instance-scoped publishers) to see them (metrics
only publish over MQTT when `metricEmission.target` is `messaging`; the default `log` target
writes a local file instead). `DataFacade` defaults an omitted sample `quality` to `Quality::Good`
(marked `qualityRaw:"unspecified"` on the wire) — pass an explicit `Quality` when your source
knows a read failed or is stale. `EventsFacade` derives the `evt/{severity}/{type}` channel from
the body's own severity + type, so the topic and body can never disagree; use
`raise_alarm`/`clear_alarm` for stateful alarms instead of one-shot `emit`. Invoke the custom verb
with a request/reply tool (e.g. MQTTX) by publishing `{"header":{"name":"set-greeting","version":
"1.0"},"body":{"greeting":"Hi there"}}` to `ecv1/{device}/{component}/cmd/set-greeting`; the
next `app` status publish reflects the new greeting. Replace all four with your own
metrics/signals/events/verbs.

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## UNS identity & topics

Topics live in the unified namespace
(`ecv1/{device}/{component}/{instance}/{class}/{channel…}`) and are minted via
`gg.uns()` (or `gg.instance(id)?.uns()`) — never hand-written. The component's
identity is config-driven: the optional top-level `hierarchy`
(`{"levels": ["site", "device"]}`) + `identity` (`{"site": "factory-1"}`) blocks in
`test-configs/config.json`; the last hierarchy level's value is always the resolved
thing name (`-t`). Messages built `.from_config(..)` carry that identity in their
envelope. The heartbeat is an automatic UNS `state` keepalive (on, every 5 s, local)
tuned by the optional `heartbeat` config block; the reserved classes
(`state`/`metric`/`cfg`/`log`) are library-owned and rejected on direct publish.

## Instance connectivity

`App::new` registers an instance-connectivity provider — the one source the `state` keepalive reads
(push) and the built-in `status` command verb reads (pull), so a console that watches and a console
that asks cannot get different answers. This scaffold owns no southbound connections, so it reports
none: the keepalive omits the `instances[]` section and `status` answers exactly what `ping`
answers.

When this component grows a connection (a device, a database, an upstream API), return one
`InstanceConnectivity` per connection: `connected` is the normalized flag every console renders a
health dot from, `state` is your own vocabulary (`ONLINE` / `CONNECTING` / `BACKOFF` / `DISABLED`),
and `attributes` is an open bag for domain data. The comment in `App::new` shows the shape.

## Deploy to Greengrass

The on-device build uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command: bash build.sh`). `build.sh` compiles the binary with the
`greengrass` feature (Greengrass IPC) and stages it per the GDK contract, then
`gdk component publish` uploads the artifact + recipe and registers the component
version in your account.

```bash
gdk component build
gdk component publish
```

> **Linux-only device build:** the `greengrass` feature compiles a C-FFI SDK and
> only builds on Linux (with `libclang`). Build on a Linux host, or cross-compile:
> `EDGECOMMONS_TARGET=x86_64-unknown-linux-gnu gdk component build`.

## Deploy to Kubernetes

Generated only when KUBERNETES is a selected target. The `Dockerfile` builds the
standalone binary into a slim, non-root image; `k8s/` holds the manifests. With
`--platform auto` the library detects KUBERNETES from the ServiceAccount token, so
no args are needed — config source defaults to CONFIGMAP, transport to MQTT (broker
config from the mounted ConfigMap), identity from the Downward API.

```bash
# 1. Build the image (the cargo git dep needs network + git auth — see Dockerfile).
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .

# 2. Make it available to the cluster: push to your registry, or load into a local kind cluster.
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
#   kind load docker-image ghcr.io/<owner>/<<COMPONENTNAME>>:latest

# 3. Set `image:` in k8s/deployment.yaml to that reference (replace REPLACE_ME), then apply.
kubectl apply -f k8s/
```

The ConfigMap is mounted as a **directory** at `/etc/edgecommons`; edit `k8s/configmap.yaml`
and `kubectl apply -f k8s/` again to hot-reload the component config in-process (no restart).

## The edgecommons dependency

`Cargo.toml` depends on the `edgecommons` crate via an **absolute path** (filled in at
generation time, `--dep-source local`, the default). This IS the local-dev override already:
Cargo resolves straight from the sibling checkout's current source, so it works against an
unpublished branch (e.g. `feat/unified-namespace`) with no extra step — unlike an
already-published component whose committed manifest carries a git-rev pin (see the `uns-bridge`
component's gitignored `.cargo/config.toml` `[patch]` for how THAT case is locally overridden
without touching the committed pin).

Regenerate with `--dep-source registry` once the library tags a real `rust-lib/vX.Y.Z` release to
switch to a git dependency (see `Cargo.toml`'s dependency comment); that pin is a release-time
item, not something this template can do today.
