# ggcommons (Rust)

Rust implementation of the Greengrass Commons library — a third implementation
alongside the Java (canonical) and Python libraries. It lets authors build AWS IoT
Greengrass v2 components in Rust while writing only business logic, bundling
configuration, messaging, metrics, heartbeat, and logging behind service traits.

> **Status: Phase 0 scaffold.** The committed near-term deliverable is the
> **standalone-mode MVP** (Phases 0–1) for Kubernetes/Docker/container deployments.
> Greengrass IPC parity (Phases 2–3) is planned follow-on. See
> [`../GGCOMMONS_RUST_PORT.md`](../GGCOMMONS_RUST_PORT.md) for the full design and plan.

## Runtime modes

- **STANDALONE** (this MVP): dual-broker MQTT (local broker + AWS IoT Core), via `rumqttc`.
- **GREENGRASS** (follow-on): Greengrass IPC, behind the `greengrass` cargo feature.

## CLI contract (shared across all three languages)

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
- `-m/--mode <MODE> [path]` — `GREENGRASS (default) | STANDALONE <messaging_config.json>`
- `-t/--thing <name>` — IoT Thing name

## Build

```bash
cargo build                      # default features (standalone)
cargo build --features greengrass
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

Apache-2.0
