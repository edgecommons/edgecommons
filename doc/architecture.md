# GGCommons (Rust) — Architecture

The Rust library is the third implementation of Greengrass Commons, alongside the
canonical Java library and the Python port. It tracks the same configuration schema
and CLI contract so all three stay at cross-language parity. See
[`../../GGCOMMONS_RUST_PORT.md`](../../GGCOMMONS_RUST_PORT.md) for the full design and
phased plan.

## Runtime modes

A component selects its runtime mode at startup with `-m/--mode`:

- **GREENGRASS** (default) — Greengrass IPC for messaging; config from the deployment.
  IPC messaging and the Greengrass config sources (`GG_CONFIG`, `SHADOW`) are behind
  the `greengrass` feature and are validated on a live Greengrass core.
- **STANDALONE** — dual-broker MQTT (local broker + AWS IoT Core) for
  Kubernetes/Docker/bare containers. Requires a messaging-config JSON file.

Both modes are implemented and functional.

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
    let messaging = gg.messaging()?; // Arc<dyn MessagingService> (Err in GREENGRASS mode for now)
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

- `standalone` (default) — STANDALONE MQTT messaging (`rumqttc`).
- `greengrass` — Greengrass IPC messaging + `GG_CONFIG`/`SHADOW` config sources
  (the SDK is a Linux-only C-FFI crate, so this feature builds only on Linux).
- `cloudwatch` — the CloudWatch metric target via the AWS SDK (heavy; off by default).
