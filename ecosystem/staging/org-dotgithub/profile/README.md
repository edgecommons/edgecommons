# edgecommons

**Build edge & IIoT components once — run them on AWS IoT Greengrass, Docker/bare hosts, or
Kubernetes.**

`edgecommons` is an ecosystem around the [`ggcommons`](https://github.com/edgecommons/ggcommons)
library: a four-language (Java · Python · Rust · TypeScript) toolkit that bundles the cross-cutting
concerns every edge component needs — config, messaging, metrics, heartbeat, logging, credentials,
parameters, and telemetry streaming — behind clean interfaces, plus a scaffolding CLI and component
templates. Write your business logic; the library handles the rest, on any platform.

## Core

| Repo | What it is |
|------|-----------|
| [`ggcommons`](https://github.com/edgecommons/ggcommons) | The library (4 languages), the `ggcommons` CLI, templates, and schema. |
| [`registry`](https://github.com/edgecommons/registry) | Machine-readable catalog of all components below. |

## Components

Discover them from the CLI (`ggcommons list-components`) or browse the [registry](https://github.com/edgecommons/registry).

### Adapters (southbound — field-device & protocol ingestion)

| Component | Lang | Protocol | Platforms |
|-----------|------|----------|-----------|
| [`opcua-adapter`](https://github.com/edgecommons/opcua-adapter) | Java | OPC UA | Greengrass · Host · Kubernetes |
| [`modbus-adapter`](https://github.com/edgecommons/modbus-adapter) | Python | Modbus (TCP / RTU / RTU-over-TCP) | Greengrass · Host · Kubernetes |

### Processors (edge compute)
_Coming soon._

### Sinks (northbound forwarding)
_Coming soon._

## Get started

```bash
pipx install ggcommons          # the scaffolding CLI
ggcommons list-components       # see what exists
ggcommons create-component -n com.example.MyAdapter -l PYTHON   # scaffold your own
```

Docs: **https://docs.ggcommons.mbreissi.com** · Build a component:
[CONTRIBUTING](https://github.com/edgecommons/.github/blob/main/CONTRIBUTING.md)
