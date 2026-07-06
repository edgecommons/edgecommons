# Logging

Logging is built on `tracing` + `tracing-subscriber`. The library installs a global
subscriber with a `fmt` layer and a **reloadable** `EnvFilter`, so the log level can
change at runtime in response to a config hot-reload.

## Initialization

`logging::init(&config, profile_format_default)` is called once during
`EdgeCommonsBuilder::build`. It is idempotent: if a global subscriber already exists (e.g. a
test installed one), init is a no-op. The level comes from `logging.level` (default `INFO`);
an unparseable level falls back to `info` rather than failing.

`profile_format_default` is the resolved platform-profile's default logging format
(`PlatformProfile::logging_format`) — `Some("json")` on KUBERNETES, `None` on GREENGRASS/HOST.
The builder threads it in because the resolved platform is known *before* the component config
loads.

```rust
use edgecommons::{config::model::Config, logging};
use serde_json::json;

let cfg = Config::from_value("c", "t", json!({ "logging": { "level": "DEBUG" } })).unwrap();
logging::init(&cfg, None); // pass Some("json") to default to the stdout-JSON sink
```

## Sinks (format selection)

The effective format is resolved once at `init` with the precedence (FR-RT-3):
**explicit `logging.rust_format` ▸ platform-profile default (`json` on KUBERNETES) ▸ library
default**. It selects one of three sinks:

| Effective format | Sink |
|------------------|------|
| `json` (case-insensitive) | **stdout-JSON** (FR-LOG-1): one JSON object per line on stdout — `timestamp`, `level`, `logger`, `message`, any structured event fields (e.g. `exception`), plus best-effort correlation fields `pod`/`namespace`/`node`/`thing` (FR-LOG-3). **stdout-only — no file rotation** (FR-LOG-2), so a read-only root FS never breaks logging. The KUBERNETES default. |
| any other token | the `{timestamp}`/`{level}`/`{target}`/`{message}` token layer (console + optional rotating file). |
| (unset, no profile default) | the default `fmt` console layer (+ optional rotating file). The GREENGRASS/HOST default — unchanged. |

Correlation fields come from the Downward-API env vars `POD_NAME` / `POD_NAMESPACE` / `NODE_NAME`
(the same vars wired in Phase 1b) and the resolved identity (`thing`); absent values are omitted.

## Runtime reconfiguration

The reload handle is stored type-erased in a `OnceLock`. On a config hot-reload, the
[`LoggingReconfigurer`] listener calls `logging::reconfigure(&config)`, which swaps
the `EnvFilter` — a cheap filter change, no re-initialization. This is applied in
exactly one place (fixing the Java M4 double-reconfigure).

```
logging.level: "INFO"  ->  (edit config)  ->  logging.level: "DEBUG"  // takes effect live
```

## Config keys

| Key | Status |
|-----|--------|
| `level` | Implemented (maps to `EnvFilter`; supports per-target directives). |
| `rust_format` | Implemented. `json` selects the stdout-JSON sink (see Sinks above); any other value is a `{timestamp}`/`{level}`/`{target}`/`{message}` token template. |
| `fileLogging` | Implemented (size-rotated file output) — **not installed under the `json` sink** (FR-LOG-2). |
| `loggers` (per-logger levels) | Reserved — map to `EnvFilter` directives. |
| `globalControl` | Reserved. |

`level` accepts a tracing `EnvFilter` string, so per-module directives work, e.g.
`"info,edgecommons::messaging=debug"`.

## Relationship to metric logs

The `log` **metric** target writes EMF JSON to its own file (see
[metric-emission.md](metric-emission.md)); that is separate from the `tracing`
application logs described here.
