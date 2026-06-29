# ggcommons (Rust)

Rust implementation of the Greengrass Commons library — a third implementation
alongside the Java (canonical) and Python libraries. It lets authors build AWS IoT
Greengrass v2 components in Rust while writing only business logic, bundling
configuration, messaging, metrics, heartbeat, and logging behind service traits.

> **Status: complete and validated on-device.** Both transports, cross-language
> parity, and Greengrass IPC (messaging, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT`)
> are implemented and have been validated against a live Greengrass core (non-root),
> including the real-time device-shadow round-trip. See
> [`../GGCOMMONS_RUST_PORT.md`](../GGCOMMONS_RUST_PORT.md) for the full design and history.

## Platform × transport runtime model

A component is described by two orthogonal axes (the legacy single `-m/--mode` axis
has been removed):

- **Platform** (`--platform`): `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`,
  which auto-detects from the environment). Phase 0 wires `GREENGRASS` and `HOST`;
  `KUBERNETES` is declared but fails fast until Phase 1.
- **Transport** (`--transport`): `IPC | MQTT`. Defaults from the platform
  (`GREENGRASS → IPC`, `HOST → MQTT`) and is independently overridable, but `IPC`
  requires `--platform GREENGRASS` (the Nucleus provides the IPC socket).
  - **MQTT**: dual-broker MQTT (local broker + AWS IoT Core), via `rumqttc`.
  - **IPC**: Greengrass IPC, behind the `greengrass` cargo feature (Linux-only).

## CLI contract (shared across all four languages)

- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT <messaging_config.json>` (default: derived from the platform)
- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `-t/--thing <name>` — IoT Thing name

The legacy `-m/--mode` mapping: `-m STANDALONE <path>` becomes
`--platform HOST --transport MQTT <path>`, and `-m GREENGRASS` becomes
`--platform GREENGRASS`.

## Build

```bash
cargo build                      # default features (standalone)
cargo build --features greengrass
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

Apache-2.0
