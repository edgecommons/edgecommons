# GGCommons Python Library

The Python implementation of the **Greengrass Commons** library (PyPI `greengrass-commons`) for
building **AWS IoT Greengrass v2** components. It bundles the cross-cutting concerns every edge
component needs — configuration, messaging, metrics, heartbeat, logging, credentials, parameters,
and telemetry streaming — so component authors write only business logic. It is one of four parallel
implementations (Java, Python, Rust, TypeScript); **Java is the canonical reference**. See the
monorepo root `README.md` and this directory's `CLAUDE.md` for the full architecture.

## Runtime modes

Every component runs in one of two modes, selected at startup via `-m/--mode`:

- **GREENGRASS** (default) — uses Greengrass IPC for messaging; reads config from the Greengrass
  deployment (`GG_CONFIG`).
- **STANDALONE** — for Kubernetes / Docker / bare hosts. Uses a dual-MQTT provider connecting
  simultaneously to a local broker and AWS IoT Core. Requires a messaging-config JSON
  (`-m STANDALONE <messaging_config.json>`).

## Subsystems

- **Configuration** — five sources (`FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT`),
  template-variable substitution, hot reload, multi-instance config, and JSON-schema validation.
  [doc](doc/configuration.md)
- **Messaging** — one interface over Greengrass IPC or STANDALONE dual-MQTT; request/reply with
  correlation; connections/subscriptions block until confirmed. [doc](doc/messaging.md)
- **Metrics** — pluggable targets: CloudWatch (EMF), cloudwatch-component, messaging, local log.
  [doc](doc/metric-emission.md)
- **Heartbeat** — periodic system metrics (CPU/memory/disk/threads/FDs). [doc](doc/heartbeat.md)
- **Logging** — Python `logging` with file rotation and per-logger levels. [doc](doc/logging.md)
- **Credentials** (`get_credentials()`) — encrypted local vault with optional AWS Secrets Manager
  sync. Opt-in: returns `None` unless a `credentials` config section is present. See `docs/CREDENTIALS.md`.
- **Parameters** (`get_parameters()`) — offline-first externalized config (env / mountedDir / AWS SSM).
  Opt-in: `None` unless a `parameters` section is present. See `docs/PARAMETERS.md`.
- **Streaming** (`get_streams()`) — high-rate telemetry streaming to Kinesis/Kafka via an embedded
  durable buffer (backed by the shared `ggstreamlog` core through a PyO3 native binding). Opt-in:
  `None` unless a `streaming` section is present. See `docs/TELEMETRY_STREAMING.md`.

## Install

```bash
pip install -r requirements.txt -r requirements-test.txt
pip install -e .
```

## Quick start

Construct the library via `GGCommonsBuilder` and read the subsystems off the returned `GGCommons`
instance. There is **no** `ggcommons.init()` facade and **no** service registry / `get_service()` —
use the concrete accessors below.

```python
import sys
from ggcommons import GGCommonsBuilder

class MyComponent:
    def main(self, args):
        gg = GGCommonsBuilder.create("com.example.MyComponent").with_args(args).build()

        config = gg.get_config_manager()     # ConfigManager
        messaging = gg.get_messaging()       # MessagingClient
        metrics = gg.get_metrics()           # MetricEmitter
        creds = gg.get_credentials()         # CredentialService or None
        params = gg.get_parameters()         # ParameterService or None
        streams = gg.get_streams()           # StreamService or None

        global_config = config.get_global_config()
        for instance_id in config.get_instance_ids():
            instance_config = config.get_instance_config(instance_id)
            # ... start instance-specific processing

if __name__ == "__main__":
    MyComponent().main(sys.argv[1:])
```

### Messaging (builder)

```python
from ggcommons.messaging import MessageBuilder

messaging = gg.get_messaging()
messaging.subscribe("requests/process", self.handle_request, 1)

message = (MessageBuilder.create("ProcessData", "1.0")
           .with_payload(payload)
           .with_config(gg.get_config_manager())
           .with_correlation_id("req-123")
           .build())
messaging.publish("requests/process", message)
```

### Metrics (builder)

```python
from ggcommons.metrics import MetricBuilder

metric = (MetricBuilder.create("data_processed")
          .with_namespace("MyApp/Metrics")
          .add_measure("count", "Count", 1)
          .build())

metrics = gg.get_metrics()
metrics.define_metric(metric)
metrics.emit_metric("data_processed", {"count": 100.0})
```

> Construct via builders (`GGCommonsBuilder`, `MessageBuilder`, `MetricBuilder`,
> `ConfigManagerBuilder`), not raw constructors. `MetricBuilder` replaces the deprecated direct
> `Metric` constructor — don't instantiate `Metric` directly.

## Configuration file example

```json
{
  "logging": {
    "level": "INFO",
    "python_format": "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    "fileLogging": { "enabled": true, "filePath": "/var/log/{ComponentName}.log", "maxFileSize": "10MB", "backupCount": 5 }
  },
  "heartbeat": { "intervalSecs": 30, "measures": { "cpu": true, "memory": true, "disk": false }, "targets": [{"type": "metric"}] },
  "metricEmission": { "target": "cloudwatch", "namespace": "MyApplication" },
  "tags": { "site": "factory-1" },
  "component": { "global": { "timeout": 5000 }, "instances": [ { "id": "main" } ] }
}
```

The config schema is the single-source `schema/ggcommons-config-schema.json` at the monorepo root
(synced into `ggcommons/resources/`). The top level is strict; subsystem sections are permissive.

## Run a component

```bash
# GREENGRASS mode (default)
python3 main.py -c GG_CONFIG -t my-thing-name
# STANDALONE mode
python3 main.py -m STANDALONE ./standalone-messaging.json -c FILE ./config.json -t my-thing-name
```

### CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG` (default), `SHADOW`, `CONFIG_COMPONENT`.
- `-m/--mode <MODE> [path]` — `GREENGRASS` (default) or `STANDALONE <messaging_config.json>`.
- `-t/--thing <name>` — IoT Thing name (takes the full string).

## Local development with MQTT

```bash
docker compose -f ../../test-infra/compose.yaml up -d   # EMQX broker (or `docker run … emqx/emqx`)
python3 main.py -m STANDALONE standalone-messaging.json -c FILE config.json -t my-device
```
Subscribe to `heartbeat/+/+` (e.g. with MQTTX) to see heartbeats; subscribe to the component's topics
to see its messages and publish to drive request/response.

## Testing

Tests are **pytest-style** (`Test*` classes, `test_*` functions); don't add `unittest.TestCase`
subclasses. There is no DI/mock-service seam in Python — test against the concrete services and
**reset the process-global statics** (`MessagingClient`, `MetricEmitter`) between tests, since their
class-level state leaks otherwise.

```bash
python -m pytest                                  # all tests (pytest.ini; log_cli=DEBUG, very verbose)
python -m pytest tests/test_builders.py::TestMessageBuilder::test_build -v   # single test
python run_pytest.py --coverage                   # coverage wrapper
python -m pytest -m "not slow and not integration and not aws"   # skip slow/AWS-dependent
```

## Requirements

- **Python** 3.9+
- **AWS IoT Greengrass** 2.0+ (for GREENGRASS mode)
- An MQTT 3.1.1 broker (for STANDALONE mode)

Key dependencies: `awsiotsdk`, `paho-mqtt`, `jsonschema`, `psutil`.

## License

Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
SPDX-License-Identifier: Apache-2.0
