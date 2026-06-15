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
| `GG_CONFIG` | `[component] [key]` | Greengrass deployment config (default key `ComponentConfig`). **Phase 2.** |
| `SHADOW` | `[name]` | IoT named device shadow. **Phase 2.** |
| `CONFIG_COMPONENT` | — | Dedicated configuration component over messaging. **Phase 2.** |

### `-m` / `--mode <MODE> [path]`

Selects the runtime mode. Default: `GREENGRASS`.

| Mode | Extra args | Meaning |
|------|-----------|---------|
| `GREENGRASS` | — | Greengrass IPC. (Messaging is Phase 2.) |
| `STANDALONE` | `<messaging_config.json>` | Dual-broker MQTT. **The path is required** — `STANDALONE` with no path is a hard error. |

### `-t` / `--thing <name>`

The IoT Thing name. Takes the **full** string value (guards a historical bug that
truncated it to a single character). When omitted, the library falls back to
`$AWS_IOT_THING_NAME`, then to `NOT_GREENGRASS`.

## Examples

```bash
# STANDALONE against a local broker, FILE config:
my-component -m STANDALONE ./standalone-messaging.json -c FILE ./config.json -t my-thing

# GREENGRASS defaults (mode + GG_CONFIG ComponentConfig):
my-component
```

## Parsing behavior

- Unknown source/mode tokens are rejected with `GgError::Cli`.
- The variadic `-c`/`-m` shape mirrors the Java `configArgs[]` array rather than
  using subcommands.
- Application-specific options can be merged onto `cli::command()` before parsing.

See `cli.rs` for the authoritative grammar and its unit tests.
