# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) and other AI coding agents when
working in this repository.

## What this is

`ggcommons` is the **Greengrass Commons** ecosystem: libraries, a scaffolding CLI, and component
templates for building **AWS IoT Greengrass v2** components. The libraries bundle the cross-cutting
concerns every edge component needs — configuration, messaging, metrics, heartbeat, logging,
credentials, parameters, and telemetry streaming — behind clean interfaces so component authors
write only business logic.

This is a **single monorepo** (one git repo at the root). The same library is implemented in
**four languages** — Java, Python, Rust, TypeScript — as deliberate mirrors of each other: the same
config schema, the same CLI contract, the same subsystem boundaries, and the same on-wire message
envelope. **Java is the canonical reference.**

| Path | What it is | Stack |
|------|-----------|-------|
| `libs/java/` | The canonical, most complete library. Maven artifact `com.breissinger:ggcommons`. | Java 25 (LTS), Maven |
| `libs/python/` | The Python port (PyPI `greengrass-commons`). **Has its own `CLAUDE.md` — read it before working here.** | Python 3.9+, setuptools |
| `libs/rust/` | The Rust port (crate `ggcommons`). | Rust (edition 2024, MSRV 1.85), Cargo |
| `libs/ts/` | The TypeScript port (npm `ggcommons`). | TypeScript 5 / Node 18+ |
| `libs/rust-streamlog/` | Shared `ggstreamlog` core: the embedded telemetry-streaming engine. All four languages use it via native bindings (Java/Panama, Python/PyO3, Node/napi-rs); Rust uses it directly. | Rust (edition 2021), Cargo |
| `cli/` | Scaffolding CLI (`ggcommons` / `ggcommons-cli`): generate, validate, build, publish, deploy, upgrade components. | Python |
| `examples/{java,python,rust,ts}/` | Worked "best-practice" example components (skeletons) that demonstrate each library. | per language |
| `templates/{java,python,rust,typescript}/` | Minimal manifest-driven starting templates the CLI copies. | per language |
| `schema/` | **Single source of truth** for the config JSON schema (`ggcommons-config-schema.json`) + sync scripts. | JSON |
| `test-infra/` | Shared integration-test infra: EMQX broker (`compose.yaml`), TLS cert generation, and the cross-language **interop** harness (`interop/`). | Docker + Python |
| `vault-test-vectors/` | Shared credentials/vault encryption conformance vectors used by all four languages. | JSON |
| `docs/` | Cross-language design docs (`CREDENTIALS.md`, `PARAMETERS.md`, `TELEMETRY_STREAMING*.md`, `GGCOMMONS_RUST_PORT.md`). | Markdown |
| `.github/workflows/` | Per-language CI + `interop`, `streaming`, `parameters-ssm`, `release`. | GitHub Actions |

**Maintain four-way parity.** The libraries mirror each other intentionally. When changing public
behavior in one, check whether the others need the matching change. See `.validation/` for the
parity register when present.

## Core concepts (shared across all four languages)

Every component runs on a **platform** with a messaging **transport**, selected at startup via
`--platform`/`--transport` (two orthogonal axes — see `docs/platform/`). Most of the architecture
exists to abstract these differences away so the same business logic runs everywhere:
- **GREENGRASS** (`--platform GREENGRASS`): uses Greengrass IPC for messaging (`--transport IPC`,
  the default for this platform); reads config from the Greengrass deployment (`GG_CONFIG`). The
  on-device, Nucleus-managed path.
- **HOST** (`--platform HOST`): for Docker / bare hosts. Defaults to `--transport MQTT`, a dual-MQTT
  provider that connects simultaneously to a local broker and to AWS IoT Core. The MQTT broker/TLS
  config is supplied either as the `--transport MQTT <messaging_config.json>` payload or via the
  active config source (`-c`).
- **KUBERNETES** (`--platform KUBERNETES`): enum value declared in Phase 0 but not yet wired — the
  Kubernetes profile and its native facilities land in Phase 1.
- **`auto`** (the default): the platform is auto-detected from the environment (Nucleus signals →
  k8s service-account token → HOST fallback); always overridable by an explicit `--platform`.

**Standard CLI contract** (identical across all four languages — keep them aligned):
- `-c/--config <SOURCE> [args...]` — one of `FILE`, `ENV`, `GG_CONFIG` (default), `SHADOW`, `CONFIG_COMPONENT`.
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`); the primary axis.
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]`; defaults from the
  platform (GREENGRASS→IPC, HOST→MQTT) and is validated (IPC is only valid on GREENGRASS). The
  legacy `-m/--mode` flag is removed and errors with guidance to `--platform`/`--transport`.
- `-t/--thing <name>` — IoT Thing name; must take the full string value (a historical bug truncated it to one char).

**Shared subsystems** (each library has parallel packages/modules for these):
- **config** — five config-source managers (`FILE`/`ENV`/`GG_CONFIG`/`SHADOW`/`CONFIG_COMPONENT`),
  template-variable substitution (`{ComponentName}`, `{ThingName}`, custom tags) with sanitization,
  hot reload, multi-instance config, and JSON-schema validation against the canonical `schema/`.
- **messaging** — one interface over two transports (Greengrass IPC vs dual-MQTT);
  connections/subscriptions block until confirmed; request/reply with correlation; per-subscription
  concurrency cap; the message envelope is identical across languages.
- **metrics** — pluggable targets: CloudWatch EMF, cloudwatch-component, messaging, local log.
- **heartbeat** — periodic system metrics (CPU/memory/disk/threads/FDs) via the metric or messaging subsystem.
- **logging** — console + optional size-rotated file logging; per-language format token (`logging.<lang>_format`).
- **credentials** — `gg.credentials()`: an encrypted local vault (envelope encryption) with optional
  central sync from AWS Secrets Manager over TES. Conformance vectors in `vault-test-vectors/`; design in `docs/CREDENTIALS.md`.
- **parameters** — `gg.parameters()`: offline-first externalized config (env / mountedDir / AWS SSM),
  reusing the credentials vault as an encrypted cache. Design in `docs/PARAMETERS.md`.
- **streaming** — `gg.streams()`: high-rate telemetry streaming with an embedded durable (or in-memory)
  buffer that drains to Kinesis/Kafka. Backed by the shared `ggstreamlog` core. Design in `docs/TELEMETRY_STREAMING.md`.

The newer subsystems (credentials, parameters, streaming) are **opt-in**: the accessor returns
null/None/an empty service unless the matching config section is present (and, in Rust, the matching
cargo feature is enabled).

## Commands

### Java library (`libs/java/`)
```bash
mvn clean package            # build + test → shaded self-contained JAR (JaCoCo enforces 90% coverage)
mvn clean package -DskipTests
mvn test -Dtest=ClassName#methodName   # single test
mvn clean install            # install to local ~/.m2
```
Compiles to Java 25; the Shade plugin produces a self-contained JAR for Greengrass deployment. The
streaming subsystem uses the Java FFM (Panama) binding to `ggstreamlog` — run with
`--enable-native-access=ALL-UNNAMED`. Live-infra tests (`GGCommonsTest`, `MessagingClientTest`) are
manual, not in the CI gate.

### Python library (`libs/python/`) — also see `libs/python/CLAUDE.md`
```bash
pip install -r requirements.txt -r requirements-test.txt && pip install -e .
python -m pytest                                  # all tests (config in pytest.ini; very verbose, log_cli=DEBUG)
python -m pytest tests/test_builders.py::TestMessageBuilder::test_build -v   # single test
python run_pytest.py --coverage                   # convenience wrapper (coverage, file/function selection)
python -m pytest -m "not slow and not integration and not aws"   # skip slow/AWS-dependent
```
`ruff`/`black` are configured but not enforced in CI — match the formatting manually.

### Rust library (`libs/rust/`) and streaming core (`libs/rust-streamlog/`)
```bash
cargo test                                         # default (standalone) build/tests — runs on any OS
cargo build --features greengrass                  # Greengrass IPC — LINUX/WSL ONLY (the native SDK won't build on Windows)
cargo clippy --all-targets
```
Off-by-default cargo features (compose as needed): `greengrass`, `cloudwatch`, `streaming` /
`streaming-kinesis` / `streaming-kafka`, `credentials` / `credentials-aws` / `credentials-pkcs11`,
`parameters` / `parameters-aws`. Building the `greengrass` feature requires Linux/WSL — see
`docs/GGCOMMONS_RUST_PORT.md` and the [[rust-greengrass-build-wsl]] note. `libs/rust-streamlog`
features: `kinesis`, `kafka`, `cabi` (C-ABI cdylib for the Java/Panama binding). Its `bench/` holds
the perf harness (`examples/loadgen.rs`, Criterion benches) — see `libs/rust-streamlog/bench/README.md`.

### TypeScript library (`libs/ts/`)
```bash
npm install
npm run build        # tsc → dist/
npm test             # vitest run
npm run coverage     # vitest run --coverage
```

### Scaffolding CLI (`cli/`)
```bash
pipx install ./cli           # or: python -m pip install ./cli  → gives `ggcommons` / `ggcommons-cli`
ggcommons doctor             # check prerequisites (git, gdk, cargo, mvn, python3, aws)
ggcommons create-component -n com.example.MyComponent -l PYTHON   # JAVA|PYTHON|RUST|TYPESCRIPT
ggcommons list-templates | validate | deploy | upgrade
```
Templates are **manifest-driven**: each ships a `ggcommons-template.json` declaring placeholder
substitutions and file renames, so adding a language needs a template, not CLI code.

### Config schema (single source — `schema/`)
The canonical config schema lives **only** in `schema/ggcommons-config-schema.json`. After editing
it, run the sync script to copy it into each library; CI fails on drift.
```bash
./schema/sync-schema.sh           # (or schema/sync-schema.ps1) → copies into libs/{java,python,rust,ts}
./schema/sync-schema.sh --check   # the drift gate CI runs (in .github/workflows/interop.yml)
```
Top level is strict (`additionalProperties:false`, `required:[component]`); subsystem sections are permissive.

### Components (skeletons & templates)
Components are packaged/deployed with the **GDK (Greengrass Development Kit)** per `gdk-config.json`
and `recipe.yaml`. Typical flow: `gdk component build` then `gdk component publish`. Run locally:
```bash
# Python example
python3 main.py --platform HOST --transport MQTT standalone-messaging.json -c FILE config.json -t my-thing
# Java example
java --enable-native-access=ALL-UNNAMED -jar target/<artifact>.jar --platform HOST --transport MQTT ./standalone-messaging.json -c FILE ./test-configs/config_2.json -t my-thing
```

### Local development with MQTT
The HOST platform with the MQTT transport (and local testing) uses a local MQTT broker standing in for Greengrass IPC:
```bash
docker compose -f test-infra/compose.yaml up -d   # EMQX (plaintext 1883 + mutual-TLS 8883)
```
Subscribe to `heartbeat/+/+` (e.g. with MQTTX) to see component heartbeats and to drive request/response topics.

### Testing & validation matrix (where each path is exercised)
All of these run from the dev machine — none is "manual / can't automate":

| Path | Where | Infra |
|------|-------|-------|
| Per-language unit/integration suites | this machine | Java (`mvn verify`, JaCoCo 90%), Python (`pytest`), Rust (`cargo test`, standalone — **no `greengrass` feature on Windows**), TS (`vitest` + coverage). Java toolchain is at `C:\Users\breis\tools\{jdk,maven}` (not on PATH). |
| **`--platform HOST`** (dual-MQTT) end-to-end | this machine | EMQX `localhost:1883` (plaintext) / `8883` (mTLS) + floci `localhost:4566`, both in Docker (`ggcommons-emqx`, `ggstreamlog-floci`). Restart them before a HOST smoke — they crash under heavy parallel-build load. |
| Rust **`greengrass` feature** build/tests (Linux-only) | **WSL** (Ubuntu, `cargo`+`cmake`+`cc`) | `wsl.exe bash -lc`, `CARGO_TARGET_DIR=/tmp`; the native GG SDK can't compile on Windows. |
| **`--platform GREENGRASS`** (IPC) on-device | **lab-5950x** (`ssh marc@192.168.1.229`, passwordless sudo; thing `lab-5950x`, us-east-1) | real Greengrass nucleus + `greengrass-cli` 2.17.0, Java 25. `gdk` is **not** installed → build the jar here, copy over, deploy with `greengrass-cli deployment create --recipeDir … --artifactDir … --merge "<Comp>=<ver>"` (`--remove` to tear down). Cloud deployments via `aws greengrassv2` (account 162499689067). |

Always unsubscribe + handle SIGTERM before exit, or a run leaks subscriptions/threads and trips the shared-connection quota.

## Conventions

- **Maintain four-way parity.** The same config schema, CLI flags, subsystem boundaries, and message
  wire format apply to all four libraries. Java is canonical. Don't diverge an API in one language
  without the matching change (or an explicit decision) in the others.
- **Construct via builders**, not raw constructors (`GGCommonsBuilder` / `GgCommonsBuilder`,
  `MessageBuilder`, `MetricBuilder`, …). `MetricBuilder` replaces the deprecated direct `Metric` constructor.
- **Backward compatibility.** Builders are the construction path in all four libs. Legacy direct
  constructors coexist **only in Java** (deprecated, still functional); **there is no `init()`
  facade in any language**. Python is builder/constructor-only; Rust/TS are builder-only greenfield.
  Don't break the old surface when adding the new one.
- **Service-interface seam (Rust/TS only).** Rust (`MessagingService`/`MetricService` traits +
  `Arc<dyn …>` injection) and TS (`IMessagingService`/`MetricService` interfaces + constructor
  injection) provide a substitutable seam. **Java and Python do not** have service interfaces or a
  `ServiceRegistry` — test against the concrete services / process-global statics
  (`MessagingClient`, `MetricEmitter`), whose state can leak across tests unless reset. (Older
  Python docs describing `ggcommons/di/` + `ggcommons/interfaces/` are wrong — those never shipped.)
- **Edit the schema in one place.** Change `schema/ggcommons-config-schema.json`, then run
  `schema/sync-schema.sh`. Never hand-edit the per-library copies.
- **Python tests are pytest-style** (`Test*` classes, `test_*` functions) — the suite was migrated
  off `unittest`; don't add new `unittest.TestCase` subclasses.
- **Per-subsystem docs** live under each library's `doc/` and the cross-language design docs under
  `docs/`. Update the relevant doc when changing a subsystem's public behavior.
- **Runtime artifacts never get committed** (`.vault`, local parameter caches, generated streams,
  TLS certs, build output).

---

This file defines strict behavioral rules for all AI coding agents (Claude Code, Junie, Cursor,
etc.) working in this project. Agents **must** follow these rules at all times.

## Karpathy's Core Recommendations (Adapted)

1. **Think before coding.** Don't assume — state assumptions explicitly. Don't hide confusion —
   surface uncertainties and tradeoffs immediately. If something is unclear, ask one clarifying
   question and wait. Prefer simpler solutions; push back on over-engineering.
2. **Simplicity first.** Implement the minimum code that solves the exact problem. Avoid speculative
   features, unnecessary abstractions, and premature optimization. If it can be done in 50 lines,
   don't write 200.
3. **Surgical changes.** Touch only the files necessary for the task. Never refactor, reformat, or
   "improve" unrelated code unless asked. Every changed line must trace back to the request.
4. **Goal-driven execution.** Restate the task as verifiable success criteria, work in iterative
   loops until all are met, and prioritize high-leverage changes.

## Mandatory verification & quality workflow

Follow this for **every** code change, in the language(s) you touched:

1. **API verification.** Before using a library/function/external API, verify its current behavior,
   parameters, and version compatibility (official docs, `cargo doc`, `mvn dependency:tree`, type
   defs, or a quick test).
2. **Build after every update.** Run the language's build/typecheck and fix all errors before
   proceeding — never leave broken code:
   - Java: `mvn -q -DskipTests compile` · Python: import / `python -m pyflakes` (or run tests) ·
     Rust: `cargo check` (`--features …` as relevant) · TS: `npm run build` (tsc).
3. **Full testing.** Write/extend unit tests (and integration tests where applicable) covering happy
   paths, edge cases, and error conditions; run the suite and fix failures:
   - Java `mvn test` · Python `python -m pytest` · Rust `cargo test` · TS `npm test`.
   Include the relevant test output in your response.
4. **Parity check.** If the change alters public behavior, the config schema, the CLI contract, or
   the message envelope, note whether the other three languages need the matching change.

## Documentation standards

Keep docs accurate and up to date with every change; document structs/classes/traits/enums, public
functions, and important constants. Update the relevant `doc/`/`docs/` page when changing a
subsystem's public behavior, and re-run `schema/sync-schema.sh` after any schema edit.

**Rust** additionally follows rustdoc conventions: `//!` module-level docs and `///` item docs on
every public module/function/type, covering purpose, parameters/return, pre/post-conditions, errors,
and a usage example where it helps. Generate/verify with `cargo doc` when appropriate. Prefer
idiomatic, safe Rust (ownership/borrowing; errors via `anyhow`/`thiserror`; async via Tokio) and
match the existing module structure.

Follow these rules strictly to produce reliable, maintainable, well-documented code.
