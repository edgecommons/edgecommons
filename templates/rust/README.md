# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Rust on top
of the `ggcommons` Rust library, generated from the GGCommons Rust component template
by the `ggcommons` CLI. It gives you the library's
standard CLI contract, configuration, logging, messaging, metrics, and heartbeat —
so you write only business logic in [`src/app.rs`](src/app.rs).

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.rs` | Entry point: builds the `ggcommons` runtime from CLI args, runs the app. |
| `src/app.rs` | Your component logic (starts as a minimal app + config-change listener). |
| `Cargo.toml` | Crate manifest. Depends on the `ggcommons` library (path dependency). |
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

## CLI contract

- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG | SHADOW | CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP)
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name

## Build & publish with the GDK (on-device)

The on-device build uses the GDK **custom** build system (`gdk-config.json` →
`custom_build_command: bash build.sh`). `build.sh` compiles the binary with the
`greengrass` feature (Greengrass IPC) and stages it per the GDK contract.

```bash
gdk component build
gdk component publish
```

> **Linux-only device build:** the `greengrass` feature compiles a C-FFI SDK and
> only builds on Linux (with `libclang`). Build on a Linux host, or cross-compile:
> `GGCOMMONS_TARGET=x86_64-unknown-linux-gnu gdk component build`.

## The ggcommons dependency

`Cargo.toml` depends on the `ggcommons` crate via an **absolute path** (filled in at
generation time). When the library is published to a git remote or a cargo registry,
replace that path dependency with the corresponding git/registry dependency.
