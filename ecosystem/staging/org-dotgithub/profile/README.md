<div align="center">

# ⚡ EdgeCommons

**Build an edge component once — run it on AWS IoT Greengrass, Docker, or Kubernetes.**

[📖 Docs](https://docs.edgecommons.mbreissi.com) · [🚀 Get started](#-get-started) · [🔌 Components](#-components)

![Languages](https://img.shields.io/badge/languages-Java_·_Python_·_Rust_·_TypeScript-blue)
![License](https://img.shields.io/badge/license-Apache--2.0-green)
![Platforms](https://img.shields.io/badge/platforms-Greengrass_·_Host_·_Kubernetes-555)

</div>

---

EdgeCommons is an ecosystem built on **`ggcommons`** — one library, four languages implemented as
deliberate mirrors. It bundles the cross-cutting concerns every edge component needs — configuration,
messaging, metrics, heartbeat, logging, credentials, parameters, and telemetry streaming — so you
write only business logic and deploy the same component anywhere.

### 🧩 Core

| Repo | What it is |
|------|------------|
| [**ggcommons**](https://github.com/edgecommons/ggcommons) | The library (Java · Python · Rust · TypeScript), the `ggcommons` CLI, templates, and config schema |
| [**registry**](https://github.com/edgecommons/registry) | Machine-readable catalog of every component |

### 🔌 Components

**Adapters** — southbound, field-device & protocol ingestion

| Component | Lang | Protocol | Platforms |
|-----------|------|----------|-----------|
| [**opcua-adapter**](https://github.com/edgecommons/opcua-adapter) | Java | OPC UA | Greengrass · Host · K8s |
| [**modbus-adapter**](https://github.com/edgecommons/modbus-adapter) | Python | Modbus (TCP / RTU / RTU-over-TCP) | Greengrass · Host · K8s |

*Processors (edge compute) and sinks (northbound) — coming soon.*

### 🚀 Get started

```bash
pipx install ggcommons          # the scaffolding CLI
ggcommons list-components       # browse the ecosystem
ggcommons create-component -n com.example.MyAdapter -l PYTHON
```

📖 Full documentation: **[docs.edgecommons.mbreissi.com](https://docs.edgecommons.mbreissi.com)**
🤝 Building a component? See [CONTRIBUTING](https://github.com/edgecommons/.github/blob/main/CONTRIBUTING.md).
