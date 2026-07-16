# Rust Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in Rust on top of the
[`edgecommons`](../../libs/rust) Rust library. It is the Rust counterpart of
`examples/java` and `examples/python`, demonstrating the
library's standard CLI contract, configuration, logging, messaging
(publish + request/reply), metrics, and heartbeat — so a component author writes
only business logic.

## What it does

All topics are minted through the **unified namespace** (UNS) builder — `gg.uns()`,
`ecv1/{device}/{component}/{instance}/{class}/{channel…}` — bound to the component's
config-driven identity (optional top-level `hierarchy` + `identity` blocks; the last
hierarchy level's value is always the resolved thing name). Topics are never
hand-written.

- **Request/reply** — subscribes to its command inbox
  `ecv1/<thing>/RustComponentSkeleton/cmd/request` and replies to each request;
  a periodic self-request demonstrates the framework request deadline
  (`EdgeCommonsError::RequestTimeout`, `messaging.requestTimeoutSeconds`).
- **Periodic publish** — publishes `ecv1/<thing>/RustComponentSkeleton/data/sample`
  every `component.global.publish_interval` seconds, emitting a `messages_published`
  metric per send (and mirrors it to IoT Core on `…/data/telemetry`).
- **Heartbeat** — automatic UNS `state` keepalive on
  `ecv1/<thing>/RustComponentSkeleton/state` (on by default, every 5 s, local);
  the enabled CPU/memory measures emit as the metric `sys` (configured by the
  optional `heartbeat` block in `recipe.yaml` / `config.json`).
- **Graceful shutdown** — runs until Ctrl-C / SIGTERM, unsubscribes, and drops the
  runtime cleanly (RAII).

> The messaging features above work on both the **HOST** platform (MQTT transport,
> against an MQTT broker) and the **GREENGRASS** platform (IPC transport, Greengrass
> IPC, built with the `greengrass` feature) — both validated against a live Greengrass core.

## Run locally (HOST platform, MQTT transport)

Start a local broker (see the workspace `CLAUDE.md`), then:

```bash
cargo run -- \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

Subscribe to `ecv1/my-thing/RustComponentSkeleton/data/sample` (or `ecv1/#`) in
an MQTT client to see published messages, and publish to
`ecv1/my-thing/RustComponentSkeleton/cmd/request` (with a `reply_to` header) to
exercise request/reply. The `state` keepalive appears on
`ecv1/my-thing/RustComponentSkeleton/state`.

## CLI contract

Same as the Java/Python skeletons:

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Build & publish with the GDK

This component uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command`: `bash build.sh`). `build.sh` compiles the binary and stages
it into `greengrass-build/` per the GDK contract (recipe → `greengrass-build/recipes/`,
artifact → `greengrass-build/artifacts/<name>/<version>/`).

```bash
gdk component build
gdk component publish
```

**Cross-compilation:** Greengrass cores typically run Linux. Build on a Linux host,
or set a Linux target you have a toolchain for:

```bash
EDGECOMMONS_TARGET=x86_64-unknown-linux-gnu gdk component build
```

The recipe declares a `linux` platform and runs the binary on the GREENGRASS platform,
reading its configuration from the deployment (`GG_CONFIG`).
