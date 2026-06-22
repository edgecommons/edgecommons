# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`ggcommons` (PyPI package `greengrass-commons`) is a library for building AWS IoT Greengrass v2 components. It bundles the cross-cutting concerns every component needs — configuration, messaging, metrics, heartbeat, logging — behind service interfaces so component authors write only business logic. A key goal of the current work (`major-rearch` branch) is bringing the Python library to feature parity with the Java version, which is why dependency injection, builders, and JSON-schema config validation were introduced.

Components built with this library run in two distinct **runtime modes**, selected at startup, and most of the architecture exists to abstract that difference away:
- **GREENGRASS** (default): uses Greengrass IPC for messaging, reads config from the Greengrass deployment.
- **STANDALONE**: for Kubernetes/Docker/bare containers. Uses a dual-MQTT provider that connects simultaneously to a local broker and to AWS IoT Core. Requires a separate messaging-config JSON file.

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
`GGCommons.__init__` (`ggcommons/ggcommons.py`) is the orchestrator and runs a fixed sequence: parse args → build config manager → set up service registry → init messaging → init metrics → init heartbeat → `complete_initialization()`. Construct it via the builder, not directly:

```python
from ggcommons import GGCommonsBuilder
gg = GGCommonsBuilder.create("com.example.MyComponent").with_args(args).build()
svc = gg.get_service(IMessagingService)
```

### Dependency injection / service interfaces — NOT present in Python
> **Correction (parity audit 2026-06-22):** earlier revisions of this file described a
> `ggcommons/di/` `ServiceRegistry` and `ggcommons/interfaces/` (`IConfigurationService`,
> `IMessagingService`, `IMetricService`) with `mock_services.py`/`testable_ggcommons.py`. **None of
> these exist in the Python source.** The substitutable service-interface seam exists only in the
> Rust and TS libraries (idiomatically); Java and Python do not have it (Java's was removed during
> remediation). In Python, depend on the concrete services and the builders below; for tests, drive
> the concretes / reset the process-global statics (`MessagingClient`, `MetricEmitter`). See the
> cross-language register `.validation/parity-remediation-plan.md`.

### Builders
Object construction goes through fluent builders, not raw constructors: `GGCommonsBuilder`, `ConfigManagerBuilder`, `MessageBuilder`, `MetricBuilder`. Note `MetricBuilder` exists specifically to avoid the deprecated direct `Metric` constructor — do not instantiate `Metric` directly.

### Configuration (`ggcommons/config/`)
`ConfigManagerBuilder.build()` dispatches on the `-c/--config` source to one of five managers, all subclassing `ConfigManager`:
`FILE` → `FileConfigManager`, `ENV` → `EnvironmentConfigManager`, `GG_CONFIG` → `GreengrassConfigManager` (default), `SHADOW` → `ShadowConfigManager`, `CONFIG_COMPONENT` → `ConfigComponentManager`.
Config supports template-variable substitution (component/thing/custom tags), hot reload via `ConfigurationChangeListener`, multi-instance components (global + per-instance config), and JSON-schema validation (`ggcommons/validation/configuration_validator.py`).

### Messaging (`ggcommons/messaging/`)
`MessagingClient.init()` picks the provider based on mode: `GreengrassIpcProvider` (IPC) or `StandaloneProvider` (dual local-MQTT + IoT Core). Both implement `MessagingProvider`. Connections and subscriptions are **blocking** — they wait for confirmation (e.g. SUBACK) before proceeding, to avoid IoT Core connection race conditions. Standalone MQTT lives under `providers/`, Greengrass IPC/IoT-Core subscription handling under `providers/greengrass/`.

### Metrics (`ggcommons/metrics/`)
`MetricEmitter` (static `init`) emits to pluggable `MetricTarget`s under `targets/`: `cloudwatch` (EMF format via `emf_helper`), `cloudwatch_component`, `messaging`, `metric_log`. Targets and component/thing dimensions are configured, not hardcoded.

### Heartbeat (`ggcommons/heartbeat/`)
`EnhancedHeartbeat` periodically emits system metrics (CPU/memory/disk/threads/FDs via `psutil`). It has services *injected* (messaging + metric) rather than reaching for globals, and can route health data through either the metric or messaging target.

### Singleton/static lifecycle caveat
`MessagingClient` and `MetricEmitter` use class-level static state (`init`/`shutdown`). This is process-global, so be careful in tests — state leaks across tests unless reset. The DI/interface layer is the testable seam; prefer injecting mock services over driving these statics.

## CLI contract

Components accept three standard arguments (custom `argparse` parsers can be merged in via the builder):
- `-c/--config <SOURCE> [args...]` — one of `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default `GG_CONFIG`).
- `-m/--mode <MODE> [path]` — `GREENGRASS` (default) or `STANDALONE <messaging_config.json>`. STANDALONE without a path is a hard error.
- `-t/--thing <name>` — IoT Thing name. Note the historical bug where this was truncated to one character; `-t`/`--thing` must take a full string value.

## Conventions

- Backward compatibility with the pre-rearch API is intended to be preserved; new patterns (builders, service interfaces) and legacy patterns can coexist. Don't break the old surface when adding the new one.
- Tests are pytest-style (`Test*` classes, `test_*` functions) — the suite was migrated off `unittest`. New tests should follow the pytest conventions, not add `unittest.TestCase` subclasses.
- Per-feature docs live in `doc/` (`architecture.md`, `messaging.md`, `configuration.md`, `metric-emission.md`, `heartbeat.md`, `logging.md`, `builder-patterns.md`, `configuration-validation.md`, `command-line-options.md`). Update the relevant doc when changing a subsystem's public behavior. (`dependency-injection.md` and `migration-guide.md` were removed — they documented a DI/service-interface layer and a legacy `init()` migration that never existed in Python; see the DI correction note above. `architecture.md` still has stale DI references and needs a rewrite.)