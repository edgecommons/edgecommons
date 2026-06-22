# ggcommons (Rust)

Rust implementation of the Greengrass Commons library — a third implementation
alongside the Java (canonical) and Python libraries. It lets authors build AWS IoT
Greengrass v2 components in Rust while writing only business logic, bundling
configuration, messaging, metrics, heartbeat, and logging behind service traits.

> **Status: complete and validated on-device.** Both runtime modes, cross-language
> parity, and Greengrass IPC (messaging, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT`)
> are implemented and have been validated against a live Greengrass core (non-root),
> including the real-time device-shadow round-trip. See
> [`../GGCOMMONS_RUST_PORT.md`](../GGCOMMONS_RUST_PORT.md) for the full design and history.

## Runtime modes

- **STANDALONE**: dual-broker MQTT (local broker + AWS IoT Core), via `rumqttc`.
- **GREENGRASS**: Greengrass IPC, behind the `greengrass` cargo feature (Linux-only).

## CLI contract (shared across all four languages)

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
