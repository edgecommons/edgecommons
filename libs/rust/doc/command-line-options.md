# Command-Line Options

The Rust library parses the **same** standard CLI contract as the Java and Python
libraries (parity is a hard requirement). Parsing is handled by `ggcommons::cli`
and surfaced as [`ParsedArgs`].

## Options

### `-c` / `--config <SOURCE> [args...]`

Selects the configuration source. The first token is the source; the rest are
source-specific. Default (when omitted): `GG_CONFIG ComponentConfig`.

| Source | Extra args | Meaning |
|--------|-----------|---------|
| `FILE` | `[path]` | JSON file (default `config.json`). Supports hot-reload. |
| `ENV` | `[var]` | JSON read from an environment variable (default `CONFIG`). |
| `GG_CONFIG` | `[component] [key]` | Greengrass deployment config (default key `ComponentConfig`). Requires the `greengrass` feature. |
| `SHADOW` | `[name]` | IoT named device shadow (name defaults to the component name). Requires the `greengrass` feature. |
| `CONFIG_COMPONENT` | — | Dedicated configuration component over messaging. |

### `--platform <PLATFORM>`

Selects the deployment platform (the primary runtime axis). Default: `auto`
(auto-detected from the environment).

| Platform | Meaning |
|----------|---------|
| `GREENGRASS` | On an AWS IoT Greengrass v2 Nucleus: defaults to IPC transport. |
| `HOST` | A plain host (Docker/bare host without a Nucleus): defaults to MQTT transport. |
| `KUBERNETES` | Declared for Phase 0; selecting it is a hard error until its profile ships in Phase 1. |
| `auto` | Auto-detect the platform from the environment (Nucleus signals → `GREENGRASS`, Kubernetes signals → `KUBERNETES`, else `HOST`). |

### `--transport <TRANSPORT> [path]`

Selects the messaging transport (the secondary runtime axis). Default: derived from
the resolved platform (`GREENGRASS → IPC`, `HOST → MQTT`).

| Transport | Extra args | Meaning |
|-----------|-----------|---------|
| `IPC` | — | Greengrass IPC messaging (requires the `greengrass` feature). Valid **only** with `--platform GREENGRASS` (the IPC lock). |
| `MQTT` | `<messaging_config.json>` | Dual-broker MQTT. The path is required when the provider is built (it is validated at provider build, not at parse time). |

### `-t` / `--thing <name>`

The IoT Thing name. Takes the **full** string value (guards a historical bug that
truncated it to a single character). When omitted, the library falls back to
`$AWS_IOT_THING_NAME`, then to `NOT_GREENGRASS`.

## Examples

```bash
# HOST + MQTT against a local broker, FILE config
# (the former `-m STANDALONE ./standalone-messaging.json`):
my-component --platform HOST --transport MQTT ./standalone-messaging.json -c FILE ./config.json -t my-thing

# GREENGRASS defaults (the former `-m GREENGRASS`; derives IPC + GG_CONFIG ComponentConfig):
my-component --platform GREENGRASS

# Auto-detect the platform from the environment (default when --platform is omitted):
my-component
```

The removed `-m/--mode` flag is rejected with guidance: `-m STANDALONE <path>` becomes
`--platform HOST --transport MQTT <path>`, and `-m GREENGRASS` becomes
`--platform GREENGRASS`.

## Parsing behavior

- Unknown source/platform/transport tokens — and the removed `-m`/`--mode` flag — are
  rejected with `GgError::Cli`.
- The variadic `-c`/`--transport` shape mirrors the Java `configArgs[]` array rather
  than using subcommands.
- Application-specific options can be merged onto `cli::command()` before parsing.

See `cli.rs` for the authoritative grammar and its unit tests.
