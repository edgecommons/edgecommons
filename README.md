# EdgeCommons — the Greengrass Commons ecosystem

EdgeCommons is a set of **libraries, a scaffolding CLI, and component templates** for building
**AWS IoT Greengrass v2** components. The libraries bundle the cross-cutting concerns every edge
component needs — **configuration, messaging, metrics, heartbeat, logging, credentials, parameters,
and telemetry streaming** — behind clean interfaces, so component authors write only business logic.
The CLI scaffolds new components from templates; the examples are worked, runnable skeletons.

The same library exists in **four languages — Java, Python, Rust, TypeScript —** as deliberate
mirrors of each other: same config schema, same CLI contract, same subsystem boundaries, same
on-wire message envelope. **Java is the canonical reference.**

> This is a **single monorepo** (one git repo at the root) with all four languages, the shared
> streaming core, the CLI, templates, the canonical config schema, and shared test infrastructure
> in one tree. Each subproject still has its own build system and CI.

## Core concept: platform × transport

Every component built with these libraries is configured along **two independent axes**, selected at
startup. Most of the architecture exists to abstract these differences away so the same business
logic runs unchanged across deployment targets:

- **`--platform <PLATFORM>`** — `GREENGRASS` | `HOST` | `KUBERNETES` | `auto` (default `auto`, which
  auto-detects the platform).
  - **GREENGRASS** — the on-device, Nucleus-managed path: reads configuration from the Greengrass
    deployment (`GG_CONFIG`) and defaults to the **IPC** transport.
  - **HOST** — a bare host / Docker container / VM. Defaults to the **MQTT** transport.
  - **KUBERNETES** — declared for first-class Kubernetes support; the wiring lands in a later phase.
- **`--transport <TRANSPORT> [path]`** — `IPC` | `MQTT [messaging_config.json]`. The default is
  derived from the platform (GREENGRASS ⇒ IPC, HOST/KUBERNETES ⇒ MQTT); **IPC is valid only on
  GREENGRASS**. The **MQTT** transport is a **dual-MQTT** provider that connects to a **local broker**
  and, optionally, **AWS IoT Core** at the same time, behind the same messaging interface, and takes
  an optional messaging-config JSON path (`--transport MQTT <messaging_config.json>`).

The standard CLI contract is identical across all four languages:
`-c/--config <SOURCE> [args]` (one of `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`,
`CONFIG_COMPONENT`; default: from the resolved platform profile — GREENGRASS → GG_CONFIG,
HOST → FILE, KUBERNETES → CONFIGMAP), `--platform <PLATFORM>`, `--transport <TRANSPORT> [path]`, and
`-t/--thing <name>`.

> **Migrating from the old `-m/--mode` flag (removed).** `-m GREENGRASS` → `--platform GREENGRASS`;
> `-m STANDALONE <messaging_config.json>` → `--platform HOST --transport MQTT <messaging_config.json>`.
> The legacy `-m/--mode` flag has been **removed** and now errors with migration guidance.

## Repository map

### Libraries (`libs/`)

| Path | What it is | Stack |
|------|-----------|-------|
| `libs/java/` | The **canonical**, most complete library. Maven artifact `com.mbreissi.edgecommons:edgecommons`. | Java 25, Maven |
| `libs/python/` | The Python port (PyPI `edgecommons`). | Python 3.9+, setuptools |
| `libs/rust/` | The Rust port (crate `edgecommons`). | Rust (edition 2024), Cargo |
| `libs/ts/` | The TypeScript port (npm `edgecommons`). | TypeScript 5 / Node 18+ |
| `libs/rust-streamlog/` | Shared **`edgestreamlog`** core: the embedded telemetry-streaming engine. All four languages use it — Rust directly, the others via native bindings (Java/Panama, Python/PyO3, Node/napi-rs in `bindings/`). | Rust, Cargo |

### Tooling & shared assets

| Path | What it is |
|------|-----------|
| `cli/` | The `edgecommons` CLI (Rust): scaffold, validate, upgrade/version, package, and release components. |
| `schema/` | **Single source of truth** for the config JSON schema (`edgecommons-config-schema.json`) + `sync-schema.{sh,ps1}` that copy it into each library (CI fails on drift). |
| `test-infra/` | Shared integration-test infra: EMQX broker (`compose.yaml`, plaintext + mutual-TLS), TLS cert generation, and the cross-language **interop** harness (`interop/`). |
| `vault-test-vectors/` | Shared credentials/vault encryption conformance vectors used by all four languages. |
| `uns-test-vectors/` | Shared Unified-Namespace conformance vectors (topics + golden envelopes) used by all four languages and the interop UNS suite. |
| `docs/` | Cross-language design docs: `CREDENTIALS.md`, `PARAMETERS.md`, `TELEMETRY_STREAMING*.md`, `EDGECOMMONS_RUST_PORT.md`, `SOUTHBOUND.md`, and the platform/UNS set under `docs/platform/` (`DESIGN-uns.md`, `UNS-CANONICAL-DESIGN.md`, …). |

### Component templates & examples

| Path | What it is |
|------|-----------|
| `templates/{java,python,rust,typescript}/` | Minimal **manifest-driven** starting templates the CLI copies (each ships a `edgecommons-template.json` declaring placeholder substitutions + file renames). |
| `examples/{java,python,rust,ts}/` | Worked "best-practice" example components (skeletons) demonstrating each library. |

## How the pieces fit together

```
        cli  ──scaffolds from──►  templates/<lang>/   (minimal manifest-driven starters)
         │                               │
         ▼                               ▼
   new component project ──depends on──►  libs/<lang>
         ▲                               ▲
  examples/<lang>/ (worked skeletons) ───┘
                                         │
        test-infra  ──exercises all libs (broker, TLS, cross-language interop)
        schema      ──one config schema, synced into every lib (drift-gated in CI)
```

## Quick start

Build a new component with the CLI (see `cli/README.md` for the full reference):

```bash
cd cli && cargo build --release              # -> cli/target/release/edgecommons
edgecommons doctor                           # check prerequisites for the platforms you target
edgecommons template list                    # the language x kind matrix
edgecommons component new -n com.example.MyComponent -l PYTHON      # JAVA|PYTHON|RUST|TYPESCRIPT
edgecommons component validate -p MyComponent
```

Run a component locally on a bare **HOST** against a local MQTT broker:

```bash
docker compose -f test-infra/compose.yaml up -d      # bring up the shared EMQX broker
python3 main.py --platform HOST --transport MQTT standalone-messaging.json -c FILE config.json -t my-thing
java --enable-native-access=ALL-UNNAMED -jar target/<artifact>.jar --platform HOST --transport MQTT ./standalone-messaging.json -c FILE ./config.json -t my-thing
```

Components are packaged and deployed with the **GDK (Greengrass Development Kit)** —
`gdk component build` then `gdk component publish`, configured per component in `gdk-config.json`
and `recipe.yaml`.

## Cross-cutting subsystems (in every library)

- **config** — five config-source managers (`FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT`),
  template-variable substitution (`{ComponentName}`, `{ThingName}`, custom tags) with sanitization,
  hot reload, multi-instance config, and JSON-schema validation against the canonical `schema/`.
- **messaging** — one interface over two transports: Greengrass **IPC** and **dual-MQTT**
  (local + IoT Core). Connections/subscriptions block until confirmed; request/reply with
  correlation and a framework deadline (`messaging.requestTimeoutSeconds`); a per-subscription
  concurrency cap; TLS (server-only or mutual). Generic component messaging config has no MQTT LWT;
  `uns-bridge` derives its private site-broker LWT internally from its resolved UNS state topic. Identical envelope across
  languages — `{header, identity, tags, body}`, with the top-level **`identity`** element
  (`{hier, path, component, instance}`) stamped on every config-built message.
- **uns** (`gg.uns()`) — the **Unified Namespace**: every component addresses the bus as
  `ecv1/{device}/{component}/{instance}/{class}[/channel]` (classes: reserved `state`/`metric`/
  `cfg`/`log` + open `data`/`evt`/`cmd`/`app`). `gg.uns()` builds and validates topics (IoT-Core
  depth-safe by construction); `gg.instance(id)` scopes topics/messages to a per-message instance;
  a fleet consumer needs only six wildcards (`ecv1/+/+/+/{state|cfg|evt|metric|data|log}`).
  Design: `docs/platform/DESIGN-uns.md`.
- **metrics** — pluggable targets: CloudWatch (EMF), cloudwatch-component, messaging (on the UNS
  `metric` class), local log, prometheus.
- **heartbeat** — the automatic UNS **`state` keepalive** (`ecv1/{device}/{component}/main/state`,
  on by default / 5 s / local) plus system measures (CPU/memory/disk/threads/FDs) emitted as the
  `sys` metric through the metric subsystem.
- **logging** — console plus optional size-rotated file logging; per-language format token.
- **credentials** (`gg.credentials()`) — encrypted local vault (envelope encryption) with optional
  central sync from AWS Secrets Manager over TES. See `docs/CREDENTIALS.md`.
- **parameters** (`gg.parameters()`) — offline-first externalized config (env / mountedDir / AWS SSM),
  using the credentials vault as an encrypted cache. See `docs/PARAMETERS.md`.
- **streaming** (`gg.streams()`) — high-rate telemetry streaming with an embedded durable (or
  in-memory) buffer that drains to Kinesis/Kafka, backed by the shared `edgestreamlog` core. See `docs/TELEMETRY_STREAMING.md`.

The newer subsystems (credentials, parameters, streaming) are **opt-in**: the accessor returns
nothing unless the matching config section is present (and, in Rust, the matching cargo feature is enabled).

## Parity

The four libraries are intentional mirrors: same config schema, CLI flags, subsystem boundaries, and
message wire format. **When changing public behavior in one, check whether the others need the
matching change** to preserve parity. Java is the canonical reference.

## Testing

- Each library has its own unit/integration suites (see its README). Broker-backed integration tests
  and the cross-language interop suite use the shared **`test-infra/`** broker + certs.
- **Cross-language interop:** `test-infra/interop/` runs a request/reply and raw publish/ingest
  round-trip for every ordered pair of languages, proving the MQTT envelope and conventions interoperate.
- **Config schema** is drift-gated: `schema/sync-schema.sh --check` runs in CI (`.github/workflows/interop.yml`).

## Working in this monorepo

There is no top-level build — each language builds independently. Start from the README in whichever
subproject you're working in, and read `CLAUDE.md` for the workspace-wide conventions. After editing
the config schema, run `schema/sync-schema.sh` so every library stays in sync.
