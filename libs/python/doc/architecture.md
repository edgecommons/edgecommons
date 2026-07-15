# EdgeCommons Python Architecture

This document describes the architecture of the EdgeCommons Python library: how `EdgeCommons` is
constructed, how the subsystems fit together, and the conventions to follow when extending it. It is
one of four parallel implementations (Java is canonical); see the monorepo root `README.md` and this
library's `CLAUDE.md`.

> **No dependency injection / service interfaces.** Earlier revisions of these docs described a
> `edgecommons/di/` `ServiceRegistry`, `edgecommons/interfaces/` (`IConfigurationService`,
> `IMessagingService`, `IMetricService`), a `ServiceFactory`, and `get_service()` / `register_service()`.
> **None of that exists in the Python source.** Depend on the concrete services via the accessors
> below. (The substitutable service-interface seam exists only in the Rust and TS libraries.)

## Construction

Always construct via the builder, never the raw constructor:

```python
from edgecommons import EdgeCommonsBuilder

gg = EdgeCommonsBuilder.create("com.example.MyComponent").with_args(args).build()
```

`EdgeCommons.__init__` (`edgecommons/edgecommons.py`) is the orchestrator and runs a fixed sequence:

1. **Argument processing** — parse the standard `-c` / `--platform` / `--transport` / `-t` contract
   (plus any app parser).
2. **Configuration** — `ConfigManagerBuilder.build()` selects the config-source manager, schema-checks
   and pre-commit-validates the initial candidate, atomically installs generation 1, then starts any
   provider watcher/subscription.
3. **Messaging** — `MessagingClient.init()` selects the provider for the resolved transport.
4. **Metrics** — `MetricEmitter.init()` wires the configured metric target(s).
5. **Heartbeat** — `EnhancedHeartbeat` starts (with messaging + metric services passed in).
6. **Opt-in subsystems** — credentials / parameters / streaming initialize **only if** their config
   section is present.
7. **`complete_initialization()`** — enables applied-configuration notifications.
8. **Command plane** — builder-configured component handlers are installed, then `CommandInbox` waits
   for transport acknowledgement before reporting `ACTIVE`.

## Accessing subsystems

Read the concrete services off the `EdgeCommons` instance:

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

### Configuration (`edgecommons/config/`)
`ConfigManagerBuilder.build()` dispatches on the `-c/--config` source to one of five managers, all
subclassing `ConfigManager`:

| Source | Manager |
|--------|---------|
| `FILE` | `FileConfigManager` |
| `ENV` | `EnvironmentConfigManager` |
| `GG_CONFIG` | `GreengrassConfigManager` |
| `SHADOW` | `ShadowConfigManager` |
| `CONFIG_COMPONENT` | `ConfigComponentManager` |

The default source comes from the resolved platform profile (GREENGRASS → GG_CONFIG,
HOST → FILE, KUBERNETES → CONFIGMAP).

Config supports template-variable substitution (component / thing / custom tags), hot reload via
`ConfigurationChangeListener`, multi-instance components (global + per-instance config), and
JSON-schema validation. Component validators run before the atomic generation swap with a 5-second
overall deadline (60-second maximum); rejected candidates never reach listeners or cfg publication.
The schema is the single-source `schema/edgecommons-config-schema.json` at the
monorepo root (synced into `edgecommons/resources/`).

```python
from edgecommons.validation import ConfigurationValidator, ConfigurationValidationException

try:
    ConfigurationValidator.validate(config)
except ConfigurationValidationException as e:
    print(f"Validation failed: {e}")
```

### Messaging (`edgecommons/messaging/`)
`MessagingClient.init()` picks the provider based on the resolved transport: `GreengrassIpcProvider`
(`IPC` transport) or `StandaloneProvider` (`MQTT` transport — dual local-MQTT + IoT Core). Both
implement `MessagingProvider`. Connections
and subscriptions are **blocking** — they wait for confirmation (e.g. SUBACK) before proceeding, to
avoid IoT Core connection races. Supports request/reply with correlation (framework deadline via
`messaging.requestTimeoutSeconds`, default 30 s); the on-wire envelope
(`header`/`identity`/`tags`/`body`) is identical across all four languages, and topics follow the
UNS grammar `ecv1/{device}/{component}/{instance}/{class}` (see [messaging.md](messaging.md)).

### Metrics (`edgecommons/metrics/`)
`MetricEmitter` (static `init`) emits to pluggable `MetricTarget`s under `targets/`: `cloudwatch`
(EMF), `cloudwatch_component`, `messaging`, and `metric_log`. Targets and component/thing dimensions
are configured, not hardcoded.

### Heartbeat (`edgecommons/heartbeat/`)
`EnhancedHeartbeat` is the library-owned liveness signal (UNS model — on by default, every 5 s): each
tick it publishes the **`state` keepalive** to `ecv1/{device}/{component}/state` (via the
reserved-publish seam) and emits the enabled system measures (CPU/memory/disk/threads/FDs via
`psutil`) as the **`sys` metric** through the metric subsystem. It has its messaging + metric services
passed in (not reached for via globals). The legacy `heartbeat.targets[]` routing is removed; see
[heartbeat.md](heartbeat.md).

### Logging (`edgecommons/logging/`)
Built on Python's standard `logging`, with file rotation, per-logger levels, and a `python_format`
token; reconfigures on config reload.

### Credentials / Parameters / Streaming
Opt-in subsystems (see `docs/CREDENTIALS.md`, `docs/PARAMETERS.md`, `docs/TELEMETRY_STREAMING.md`):
an encrypted local vault (`get_credentials()`), offline-first externalized config
(`get_parameters()`), and high-rate telemetry streaming to Kinesis/Kafka via the shared `edgestreamlog`
core through a PyO3 binding (`get_streams()`).

## Builders

Object construction goes through fluent builders, not raw constructors: `EdgeCommonsBuilder`,
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
