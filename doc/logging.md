# Logging

Logging is built on `tracing` + `tracing-subscriber`. The library installs a global
subscriber with a `fmt` layer and a **reloadable** `EnvFilter`, so the log level can
change at runtime in response to a config hot-reload.

## Initialization

`logging::init(&config)` is called once during `GgCommonsBuilder::build`. It is
idempotent: if a global subscriber already exists (e.g. a test installed one), init is
a no-op. The level comes from `logging.level` (default `INFO`); an unparseable level
falls back to `info` rather than failing.

```rust
use ggcommons::{config::model::Config, logging};
use serde_json::json;

let cfg = Config::from_value("c", "t", json!({ "logging": { "level": "DEBUG" } })).unwrap();
logging::init(&cfg);
```

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
| `format` | Reserved (the `fmt` layer's default format is used today). |
| `fileLogging` | Reserved — Phase 3 file logging with rotation. |
| `loggers` (per-logger levels) | Reserved — map to `EnvFilter` directives. |
| `globalControl` | Reserved. |

`level` accepts a tracing `EnvFilter` string, so per-module directives work, e.g.
`"info,ggcommons::messaging=debug"`.

## Relationship to metric logs

The `log` **metric** target writes EMF JSON to its own file (see
[metric-emission.md](metric-emission.md)); that is separate from the `tracing`
application logs described here.
