# GGCommons — the Greengrass Commons ecosystem

GGCommons is a set of **libraries, a scaffolding CLI, and component templates** for building
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

## Core concept: two runtime modes

Every component built with these libraries runs in one of two modes, selected at startup via
`-m/--mode`. Most of the architecture exists to abstract this difference away so the same business
logic runs in both:

- **GREENGRASS** (default) — uses Greengrass **IPC** for messaging and reads configuration from the
  Greengrass deployment (`GG_CONFIG`). The on-device, Nucleus-managed path.
- **STANDALONE** — for containers / Kubernetes / bare hosts. Uses a **dual-MQTT** provider that
  connects to a **local broker** and, optionally, **AWS IoT Core** at the same time, behind the same
  messaging interface. Requires a separate messaging-config JSON (`-m STANDALONE <messaging_config.json>`).

The standard CLI contract is identical across all four languages:
`-c/--config <SOURCE> [args]` (one of `FILE`, `ENV`, `GG_CONFIG` (default), `SHADOW`,
`CONFIG_COMPONENT`), `-m/--mode <GREENGRASS|STANDALONE [path]>`, and `-t/--thing <name>`.

## Repository map

### Libraries (`libs/`)

| Path | What it is | Stack |
|------|-----------|-------|
| `libs/java/` | The **canonical**, most complete library. Maven artifact `com.breissinger:ggcommons`. | Java 25, Maven |
| `libs/python/` | The Python port (PyPI `greengrass-commons`). | Python 3.9+, setuptools |
| `libs/rust/` | The Rust port (crate `ggcommons`). | Rust (edition 2024), Cargo |
| `libs/ts/` | The TypeScript port (npm `ggcommons`). | TypeScript 5 / Node 18+ |
| `libs/rust-streamlog/` | Shared **`ggstreamlog`** core: the embedded telemetry-streaming engine. All four languages use it — Rust directly, the others via native bindings (Java/Panama, Python/PyO3, Node/napi-rs in `bindings/`). | Rust, Cargo |

### Tooling & shared assets

| Path | What it is |
|------|-----------|
| `cli/` | Scaffolding CLI (`ggcommons` / `ggcommons-cli`): generate, validate, build, publish, deploy, and upgrade components. |
| `schema/` | **Single source of truth** for the config JSON schema (`ggcommons-config-schema.json`) + `sync-schema.{sh,ps1}` that copy it into each library (CI fails on drift). |
| `test-infra/` | Shared integration-test infra: EMQX broker (`compose.yaml`, plaintext + mutual-TLS), TLS cert generation, and the cross-language **interop** harness (`interop/`). |
| `vault-test-vectors/` | Shared credentials/vault encryption conformance vectors used by all four languages. |
| `docs/` | Cross-language design docs: `CREDENTIALS.md`, `PARAMETERS.md`, `TELEMETRY_STREAMING*.md`, `GGCOMMONS_RUST_PORT.md`. |

### Component templates & examples

| Path | What it is |
|------|-----------|
| `templates/{java,python,rust,typescript}/` | Minimal **manifest-driven** starting templates the CLI copies (each ships a `ggcommons-template.json` declaring placeholder substitutions + file renames). |
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
pipx install ./cli                           # or: python -m pip install ./cli
ggcommons doctor                             # check prerequisites (git, gdk, cargo, mvn, python3, aws)
ggcommons create-component -n com.example.MyComponent -l PYTHON   # JAVA|PYTHON|RUST|TYPESCRIPT
```

Run a component locally in STANDALONE mode against a local MQTT broker:

```bash
docker compose -f test-infra/compose.yaml up -d      # bring up the shared EMQX broker
python3 main.py -m STANDALONE standalone-messaging.json -c FILE config.json -t my-thing
java --enable-native-access=ALL-UNNAMED -jar target/<artifact>.jar -m STANDALONE ./standalone-messaging.json -c FILE ./config.json -t my-thing
```

Components are packaged and deployed with the **GDK (Greengrass Development Kit)** —
`gdk component build` then `gdk component publish`, configured per component in `gdk-config.json`
and `recipe.yaml`.

## Cross-cutting subsystems (in every library)

- **config** — five config-source managers (`FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT`),
  template-variable substitution (`{ComponentName}`, `{ThingName}`, custom tags) with sanitization,
  hot reload, multi-instance config, and JSON-schema validation against the canonical `schema/`.
- **messaging** — one interface over two providers: Greengrass **IPC** and STANDALONE **dual-MQTT**
  (local + IoT Core). Connections/subscriptions block until confirmed; request/reply with
  correlation; a per-subscription concurrency cap; TLS (server-only or mutual). Identical envelope across languages.
- **metrics** — pluggable targets: CloudWatch (EMF), cloudwatch-component, messaging, local log.
- **heartbeat** — periodic system metrics (CPU/memory/disk/threads/FDs) routed through the metric or messaging subsystem.
- **logging** — console plus optional size-rotated file logging; per-language format token.
- **credentials** (`gg.credentials()`) — encrypted local vault (envelope encryption) with optional
  central sync from AWS Secrets Manager over TES. See `docs/CREDENTIALS.md`.
- **parameters** (`gg.parameters()`) — offline-first externalized config (env / mountedDir / AWS SSM),
  using the credentials vault as an encrypted cache. See `docs/PARAMETERS.md`.
- **streaming** (`gg.streams()`) — high-rate telemetry streaming with an embedded durable (or
  in-memory) buffer that drains to Kinesis/Kafka, backed by the shared `ggstreamlog` core. See `docs/TELEMETRY_STREAMING.md`.

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
