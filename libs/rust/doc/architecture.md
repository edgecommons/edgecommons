# GGCommons (Rust) — Architecture

The Rust library is the third implementation of Greengrass Commons, alongside the
canonical Java library and the Python port. It tracks the same configuration schema
and CLI contract so all three stay at cross-language parity. See
[`../../GGCOMMONS_RUST_PORT.md`](../../GGCOMMONS_RUST_PORT.md) for the full design and
phased plan.

## Platform × transport runtime model

A component is described by two orthogonal axes selected at startup (the legacy single
`-m/--mode` axis has been removed): `--platform GREENGRASS|HOST|KUBERNETES|auto`
(default `auto`) and `--transport IPC|MQTT` (default: derived from the platform).

- **`--platform GREENGRASS`** (transport defaults to `IPC`) — Greengrass IPC for
  messaging; config from the deployment. IPC messaging and the Greengrass config
  sources (`GG_CONFIG`, `SHADOW`) are behind the `greengrass` feature and are validated
  on a live Greengrass core.
- **`--platform HOST`** (transport defaults to `MQTT`) — dual-broker MQTT (local broker
  + AWS IoT Core) for Docker/bare containers. The MQTT transport requires a
  messaging-config JSON file (`--transport MQTT <messaging_config.json>`).
- **`--platform KUBERNETES`** — declared for Phase 0; selecting it fails fast until its
  profile ships in Phase 1.

`IPC` is valid only on `--platform GREENGRASS` (the IPC lock). The former
`-m STANDALONE <path>` is now `--platform HOST --transport MQTT <path>`, and
`-m GREENGRASS` is `--platform GREENGRASS`. Both Phase-0 platforms are implemented and
functional.

## The runtime object

[`GgCommons`] is built by [`GgCommonsBuilder`] and owns the wired services. It is the
only supported construction path:

```rust
use ggcommons::prelude::*;

#[tokio::main]
async fn main() -> ggcommons::Result<()> {
    let gg = GgCommonsBuilder::new("com.example.MyComponent")
        .args(std::env::args_os())
        .build()
        .await?;

    let cfg = gg.config();        // Arc<Config> snapshot
    let metrics = gg.metrics();   // Arc<dyn MetricService>
    let messaging = gg.messaging()?; // Arc<dyn MessagingService> (Err on the IPC transport without the `greengrass` feature)
    Ok(())
}
```

Services are exposed as trait objects (`Arc<dyn _>`) — the testable seam. Inject
fakes in tests rather than driving process-global state.

## Subsystems

| Subsystem | Module | Guide |
|-----------|--------|-------|
| CLI contract | `cli` | [command-line-options.md](command-line-options.md) |
| Configuration (sources, hot-reload, validation, templating) | `config` | [configuration.md](configuration.md) |
| Messaging (providers + service, request/reply) | `messaging` | [messaging.md](messaging.md) |
| Metrics (EMF + targets) | `metrics` | [metric-emission.md](metric-emission.md) |
| Heartbeat (system metrics) | `heartbeat` | [heartbeat.md](heartbeat.md) |
| Logging (`tracing`, runtime reconfiguration) | `logging` | [logging.md](logging.md) |

## Design principles (vs. the Java baseline)

The Rust port starts from a more correct baseline by construction:

- **No `process::exit` in the library** — everything is `Result<T, GgError>`.
- **RAII shutdown** — dropping `GgCommons` stops the heartbeat task, cancels the
  config watcher, and closes MQTT clients. There is no `close()` to forget.
- **Atomic config snapshots** — config is an immutable `Arc<Config>` published via
  `arc_swap::ArcSwap`; readers never see a torn update.
- **Request/reply built once** over a transport trait, so it cannot diverge between
  transports (the Java C1/C2 class of bug is structurally impossible).
- **No silent credential fallback** — an IoT Core TLS credential failure is a hard
  error; the client never connects unauthenticated.

See §11 of the design doc for the full mapping from Java code-review findings to
their Rust resolutions.

## Cargo features

- `standalone` (default) — the dual-broker MQTT transport (`rumqttc`).
- `greengrass` — Greengrass IPC messaging + `GG_CONFIG`/`SHADOW` config sources
  (the SDK is a Linux-only C-FFI crate, so this feature builds only on Linux).
- `cloudwatch` — the CloudWatch metric target via the AWS SDK (heavy; off by default).
