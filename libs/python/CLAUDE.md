# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`edgecommons` (PyPI package `edgecommons`) is a library for building AWS IoT Greengrass v2 components. It bundles the cross-cutting concerns every component needs — configuration, messaging, metrics, heartbeat, logging — behind service interfaces so component authors write only business logic. A key goal of the current work (`major-rearch` branch) is bringing the Python library to feature parity with the Java version, which is why dependency injection, builders, and JSON-schema config validation were introduced.

Components built with this library are configured along two axes — a **platform** and a **transport** — selected at startup, and most of the architecture exists to abstract that difference away:
- **`--platform`** (`GREENGRASS` | `HOST` | `KUBERNETES` | `auto`, default `auto` which auto-detects): `GREENGRASS` uses Greengrass IPC for messaging and reads config from the Greengrass deployment; `HOST` is a plain host / container without a Nucleus; `KUBERNETES` is declared but not yet wired (Phase 1).
- **`--transport`** (`IPC` | `MQTT [messaging_config.json]`, default derived from the platform — `GREENGRASS` ⇒ `IPC`, `HOST`/`KUBERNETES` ⇒ `MQTT`): `IPC` is native Greengrass Nucleus IPC (valid **only** on `--platform GREENGRASS`); `MQTT` uses a dual-MQTT provider that connects simultaneously to a local broker and to AWS IoT Core, and requires a separate messaging-config JSON file.

The legacy `-m/--mode` flag has been removed: the old `-m GREENGRASS` is now `--platform GREENGRASS` and the old `-m STANDALONE <path>` is now `--platform HOST --transport MQTT <path>`.

## Commands

```bash
# Install (editable, with test deps)
pip install -r requirements.txt -r requirements-test.txt
pip install -e .

# Run all tests (pytest config lives in pytest.ini)
python -m pytest

# Convenience wrapper around pytest (coverage, file/function selection)
python run_pytest.py --coverage
python run_pytest.py -f tests/test_builders.py --function test_message_builder

# Single file / single test directly
python -m pytest tests/test_builders.py
python -m pytest tests/test_builders.py::TestMessageBuilder::test_build -v

# Skip slow / integration / AWS-dependent tests (markers defined in pytest.ini)
python -m pytest -m "not slow and not integration and not aws"
```

Note: `pytest.ini` sets `log_cli = true` at `DEBUG`, so test runs are very verbose by design.

There is no enforced linter in CI (the ruff/tox steps in `.gitlab-ci.yml` are commented out), but `ruff` and `black` (target py39–py311) are configured, so match that formatting. CI's only active job builds the wheel and publishes to the GitLab PyPI registry.

## Architecture

### Initialization flow
`EdgeCommons.__init__` (`edgecommons/edgecommons.py`) is the orchestrator and runs a fixed sequence: parse args → build config manager → set up service registry → init messaging → init metrics → init heartbeat → `complete_initialization()`. Construct it via the builder, not directly:

```python
from edgecommons import EdgeCommonsBuilder
gg = EdgeCommonsBuilder.create("com.example.MyComponent").with_args(args).build()
svc = gg.get_service(IMessagingService)
```

### Dependency injection / service interfaces — NOT present in Python
> **Correction (parity audit 2026-06-22):** earlier revisions of this file described a
> `edgecommons/di/` `ServiceRegistry` and `edgecommons/interfaces/` (`IConfigurationService`,
> `IMessagingService`, `IMetricService`) with `mock_services.py`/`testable_edgecommons.py`. **None of
> these exist in the Python source.** The substitutable service-interface seam exists only in the
> Rust and TS libraries (idiomatically); Java and Python do not have it (Java's was removed during
> remediation). In Python, depend on the concrete services and the builders below; for tests, drive
> the concretes / reset the process-global statics (`MessagingClient`, `MetricEmitter`). See the
> cross-language register `.validation/parity-remediation-plan.md`.

### Builders
Object construction goes through fluent builders, not raw constructors: `EdgeCommonsBuilder`, `ConfigManagerBuilder`, `MessageBuilder`, `MetricBuilder`. Note `MetricBuilder` exists specifically to avoid the deprecated direct `Metric` constructor — do not instantiate `Metric` directly.

### Configuration (`edgecommons/config/`)
`ConfigManagerBuilder.build()` dispatches on the `-c/--config` source to one of five managers, all subclassing `ConfigManager`:
`FILE` → `FileConfigManager`, `ENV` → `EnvironmentConfigManager`, `GG_CONFIG` → `GreengrassConfigManager`, `SHADOW` → `ShadowConfigManager`, `CONFIG_COMPONENT` → `ConfigComponentManager`. The default source comes from the resolved platform profile (GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
Config supports template-variable substitution (component/thing/custom tags), hot reload via `ConfigurationChangeListener`, multi-instance components (global + per-instance config), and JSON-schema validation (`edgecommons/validation/configuration_validator.py`).

### Messaging (`edgecommons/messaging/`)
`MessagingClient.init()` picks the provider based on the resolved transport: `GreengrassIpcProvider` (`IPC` transport) or `StandaloneProvider` (`MQTT` transport — dual local-MQTT + IoT Core). Both implement `MessagingProvider`. Connections and subscriptions are **blocking** — they wait for confirmation (e.g. SUBACK) before proceeding, to avoid IoT Core connection race conditions. Standalone MQTT lives under `providers/`, Greengrass IPC/IoT-Core subscription handling under `providers/greengrass/`.

### Metrics (`edgecommons/metrics/`)
`MetricEmitter` (static `init`) emits to pluggable `MetricTarget`s under `targets/`: `cloudwatch` (EMF format via `emf_helper`), `cloudwatch_component`, `messaging`, `metric_log`. Targets and component/thing dimensions are configured, not hardcoded.

### Heartbeat (`edgecommons/heartbeat/`)
`EnhancedHeartbeat` is the library-owned liveness signal (UNS model — on by default, every 5 s): each tick it publishes the **`state` keepalive** to the UNS topic `ecv1/{device}/{component}/main/state` (through the library-internal `MessagingClient._publish_reserved*` method — the `state` class is reserved) and emits the enabled system measures (CPU/memory/disk/threads/FDs via `psutil`) as the **`sys` metric** through the metric subsystem. It has services *injected* (messaging + metric) rather than reaching for globals. The legacy `heartbeat.targets[]` config is removed — the section is `{enabled, intervalSecs, measures, destination}` (`destination` governs only the keepalive: `local`|`northbound`). Consume heartbeats by subscribing `ecv1/+/+/+/state`.

### Singleton/static lifecycle caveat
`MessagingClient` and `MetricEmitter` use class-level static state (`init`/`shutdown`). This is process-global, so be careful in tests — state leaks across tests unless reset. The DI/interface layer is the testable seam; prefer injecting mock services over driving these statics.

## CLI contract

Components accept these standard arguments (custom `argparse` parsers can be merged in via the builder):
- `-c/--config <SOURCE> [args...]` — one of `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
- `--platform <PLATFORM>` — `GREENGRASS` | `HOST` | `KUBERNETES` | `auto` (default `auto`, which auto-detects from the environment). `KUBERNETES` is declared but not yet wired (Phase 1) and fails fast if selected.
- `--transport <TRANSPORT> [path]` — `IPC` | `MQTT [messaging_config.json]` (default derived from the resolved platform: `GREENGRASS` ⇒ `IPC`, `HOST`/`KUBERNETES` ⇒ `MQTT`). `IPC` is valid **only** on `--platform GREENGRASS`. The `MQTT` messaging-config path is required when the MQTT provider is actually built.
- `-t/--thing <name>` — IoT Thing name. Note the historical bug where this was truncated to one character; `-t`/`--thing` must take a full string value.

The legacy `-m/--mode` flag has been removed and now errors with guidance to the new flags: `-m GREENGRASS` → `--platform GREENGRASS`; `-m STANDALONE <path>` → `--platform HOST --transport MQTT <path>`.

## Conventions

- Backward compatibility with the pre-rearch API is intended to be preserved; new patterns (builders, service interfaces) and legacy patterns can coexist. Don't break the old surface when adding the new one.
- Wire-format or messaging-behavior changes in Python are core contract changes: update the
  Java/Rust/TS mirrors, extend `../../test-infra/interop/` across all four language nodes, and run
  the full local MQTT interop matrix before calling the work complete. Every affected language must
  act as producer and consumer; for binary bodies, assert exact byte equality.
- If that wire/messaging behavior is reachable through Greengrass IPC, also deploy the four language
  skeleton/components on `lab-5950x` and verify the same behavior through real IPC. Binary message
  changes explicitly require both local MQTT and Greengrass IPC cross-language interop. Unit tests,
  mocked IPC tests, single-language component runs, and RUNNING-state smoke checks are not enough.
- New Python capabilities or Greengrass-reachable behavior changes must also be deployed and exercised
  as a real Greengrass component on `lab-5950x`; if a feature is not Greengrass-applicable, document
  why and run a baseline deployed-component regression.
- Tests are pytest-style (`Test*` classes, `test_*` functions) — the suite was migrated off `unittest`. New tests should follow the pytest conventions, not add `unittest.TestCase` subclasses.
- Per-feature docs live in `doc/` (`architecture.md`, `messaging.md`, `configuration.md`, `metric-emission.md`, `heartbeat.md`, `logging.md`, `builder-patterns.md`, `configuration-validation.md`, `command-line-options.md`). Update the relevant doc when changing a subsystem's public behavior. (`dependency-injection.md` and `migration-guide.md` were removed — they documented a DI/service-interface layer and a legacy `init()` migration that never existed in Python; see the DI correction note above. `architecture.md` still has stale DI references and needs a rewrite.)
