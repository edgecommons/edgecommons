# GGCommons Python Architecture

This document describes the architecture of the GGCommons Python library: how `GGCommons` is
constructed, how the subsystems fit together, and the conventions to follow when extending it. It is
one of four parallel implementations (Java is canonical); see the monorepo root `README.md` and this
library's `CLAUDE.md`.

> **No dependency injection / service interfaces.** Earlier revisions of these docs described a
> `ggcommons/di/` `ServiceRegistry`, `ggcommons/interfaces/` (`IConfigurationService`,
> `IMessagingService`, `IMetricService`), a `ServiceFactory`, and `get_service()` / `register_service()`.
> **None of that exists in the Python source.** Depend on the concrete services via the accessors
> below. (The substitutable service-interface seam exists only in the Rust and TS libraries.)

## Construction

Always construct via the builder, never the raw constructor:

```python
from ggcommons import GGCommonsBuilder

gg = GGCommonsBuilder.create("com.example.MyComponent").with_args(args).build()
```

`GGCommons.__init__` (`ggcommons/ggcommons.py`) is the orchestrator and runs a fixed sequence:

1. **Argument processing** — parse the standard `-c` / `--platform` / `--transport` / `-t` contract
   (plus any app parser).
2. **Configuration** — `ConfigManagerBuilder.build()` selects the config-source manager and loads +
   validates the config.
3. **Messaging** — `MessagingClient.init()` selects the provider for the resolved transport.
4. **Metrics** — `MetricEmitter.init()` wires the configured metric target(s).
5. **Heartbeat** — `EnhancedHeartbeat` starts (with messaging + metric services passed in).
6. **Opt-in subsystems** — credentials / parameters / streaming initialize **only if** their config
   section is present.
7. **`complete_initialization()`** — enables configuration-change notifications.

## Accessing subsystems

Read the concrete services off the `GGCommons` instance:

```python
config     = gg.get_config_manager()   # ConfigManager
messaging  = gg.get_messaging()        # MessagingClient
metrics    = gg.get_metrics()          # MetricEmitter
creds      = gg.get_credentials()      # CredentialService or None
params     = gg.get_parameters()       # ParameterService or None
streams    = gg.get_streams()          # StreamService or None
```

The three newest accessors return `None` unless their config section exists.

## Subsystems

### Configuration (`ggcommons/config/`)
`ConfigManagerBuilder.build()` dispatches on the `-c/--config` source to one of five managers, all
subclassing `ConfigManager`:

| Source | Manager |
|--------|---------|
| `FILE` | `FileConfigManager` |
| `ENV` | `EnvironmentConfigManager` |
| `GG_CONFIG` (default) | `GreengrassConfigManager` |
| `SHADOW` | `ShadowConfigManager` |
| `CONFIG_COMPONENT` | `ConfigComponentManager` |

Config supports template-variable substitution (component / thing / custom tags), hot reload via
`ConfigurationChangeListener`, multi-instance components (global + per-instance config), and
JSON-schema validation. The schema is the single-source `schema/ggcommons-config-schema.json` at the
monorepo root (synced into `ggcommons/resources/`).

```python
from ggcommons.validation import ConfigurationValidator, ConfigurationValidationException

try:
    ConfigurationValidator.validate(config)
except ConfigurationValidationException as e:
    print(f"Validation failed: {e}")
```

### Messaging (`ggcommons/messaging/`)
`MessagingClient.init()` picks the provider based on the resolved transport: `GreengrassIpcProvider`
(`IPC` transport) or `StandaloneProvider` (`MQTT` transport — dual local-MQTT + IoT Core). Both
implement `MessagingProvider`. Connections
and subscriptions are **blocking** — they wait for confirmation (e.g. SUBACK) before proceeding, to
avoid IoT Core connection races. Supports request/reply with correlation; the on-wire envelope is
identical across all four languages.

### Metrics (`ggcommons/metrics/`)
`MetricEmitter` (static `init`) emits to pluggable `MetricTarget`s under `targets/`: `cloudwatch`
(EMF), `cloudwatch_component`, `messaging`, and `metric_log`. Targets and component/thing dimensions
are configured, not hardcoded.

### Heartbeat (`ggcommons/heartbeat/`)
`EnhancedHeartbeat` periodically emits system metrics (CPU/memory/disk/threads/FDs via `psutil`). It
has its messaging + metric services passed in (not reached for via globals) and can route health data
through either the metric or messaging target.

### Logging (`ggcommons/logging/`)
Built on Python's standard `logging`, with file rotation, per-logger levels, and a `python_format`
token; reconfigures on config reload.

### Credentials / Parameters / Streaming
Opt-in subsystems (see `docs/CREDENTIALS.md`, `docs/PARAMETERS.md`, `docs/TELEMETRY_STREAMING.md`):
an encrypted local vault (`get_credentials()`), offline-first externalized config
(`get_parameters()`), and high-rate telemetry streaming to Kinesis/Kafka via the shared `ggstreamlog`
core through a PyO3 binding (`get_streams()`).

## Builders

Object construction goes through fluent builders, not raw constructors: `GGCommonsBuilder`,
`ConfigManagerBuilder`, `MessageBuilder`, `MetricBuilder`. `MetricBuilder` exists specifically to
avoid the deprecated direct `Metric` constructor — do not instantiate `Metric` directly.

```python
message = (MessageBuilder.create("heartbeat", "1.0")
           .with_payload(data).with_config(config_manager).with_correlation_id("12345").build())

metric = (MetricBuilder.create("cpu_usage")
          .with_namespace("MyApp/Metrics").add_measure("usage", "Percent", 1).build())
```

## Static-lifecycle caveat (testing)

`MessagingClient` and `MetricEmitter` use class-level static state (`init`/`shutdown`). This is
process-global, so it **leaks across tests unless reset**. There is no DI/mock-service seam in
Python — test against the concrete services and reset these statics between tests. Tests are
pytest-style; don't add `unittest.TestCase` subclasses.
