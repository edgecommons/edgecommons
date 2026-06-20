# GGCommons — the Greengrass Commons ecosystem

GGCommons is a set of **libraries, a scaffolding CLI, and component templates** for building
**AWS IoT Greengrass v2** components. The libraries bundle the cross-cutting concerns every
edge component needs — **configuration, messaging, metrics, heartbeat, logging** — behind
service interfaces, so component authors write only business logic. The CLI scaffolds new
components from templates; the skeletons are worked, runnable examples.

The same library exists in three languages (**Java, Python, Rust**) as deliberate mirrors of
each other: same config schema, same CLI contract, same subsystem boundaries.

> This directory is a **workspace of independent git repositories**, not a single buildable
> repo — the workspace root is not itself a git repo. Each subproject below has its own repo,
> build system, CI, and README.

## Core concept: two runtime modes

Every component built with these libraries runs in one of two modes, selected at startup via
`-m/--mode`. Most of the architecture exists to abstract this difference away so the same
business logic runs in both:

- **GREENGRASS** (default) — uses Greengrass **IPC** for messaging and reads configuration from
  the Greengrass deployment. This is the on-device, Nucleus-managed path.
- **STANDALONE** — for containers / Kubernetes / bare hosts. Uses a **dual-MQTT** provider that
  connects to a **local broker** and, optionally, **AWS IoT Core** at the same time, behind the
  same messaging interface. Requires a separate messaging-config JSON
  (`-m STANDALONE <messaging_config.json>`).

The standard CLI contract is identical across all three languages:
`-c/--config <SOURCE> [args]` (one of `FILE`, `ENV`, `GG_CONFIG` (default), `SHADOW`,
`CONFIG_COMPONENT`), `-m/--mode <GREENGRASS|STANDALONE [path]>`, and `-t/--thing <name>`.

## Repository map

### Libraries (the core)

| Repo | What it is | Stack |
|------|-----------|-------|
| `ggcommons-java-lib/` | The **canonical**, most complete library. Maven/Java artifact `com.aws.proserve:ggcommons`. | Java 11+ (built on JDK 25, language target 21), Maven |
| `ggcommons-python-lib/` | The Python port (PyPI `greengrass-commons`), at feature parity with Java. | Python 3.9+, setuptools |
| `ggcommons-rust-lib/` | The Rust port (crate `ggcommons`), at parity with Java/Python. | Rust (edition 2024), Cargo |

### Tooling

| Repo | What it is | Stack |
|------|-----------|-------|
| `ggcommons-cli/` | Scaffolding CLI (`ggcommons` / `ggcommons-cli`) that generates new components from templates and helps validate/build/publish/deploy/upgrade them. | Python |
| `ggcommons-test-infra/` | Shared integration-test infrastructure used by **all three** libraries: an EMQX broker (`compose.yaml`, plaintext + mutual-TLS listeners), TLS cert generation, and the **cross-language interop** harness (`interop/`). | Docker + Python |

### Component templates (minimal starting points the CLI copies)

| Repo | What it is | Stack |
|------|-----------|-------|
| `java-componen-template/` | Minimal Java component template *(note the typo in the directory name)*. | Java / Maven |
| `python-component-template/` | Minimal Python component template. | Python |
| `rust-component-template/` | Minimal Rust component template. | Rust |

Templates are **manifest-driven**: each ships a `ggcommons-template.json` declaring the
placeholder substitutions and file renames, so adding a language needs a template, not CLI code.

### Component skeletons (worked "best-practice" examples)

| Repo | What it is | Stack |
|------|-----------|-------|
| `java-component-skeleton/` | A fuller worked example component demonstrating the library. | Java |
| `python-component-skeleton/` | Worked example component (also carries local integration tests). | Python |
| `rust-component-skeleton/` | Worked example component. | Rust |

### Root docs

- `CLAUDE.md` — guidance for AI coding agents and contributors working across the workspace.
- `GGCOMMONS_RUST_PORT.md` — design notes and bug catalog for the Rust port.

## How the pieces fit together

```
                ggcommons-cli  ──scaffolds from──►  *-component-template/   (minimal starters)
                      │                                     │
                      ▼                                     ▼
              new component project ──depends on──►  ggcommons-{java,python,rust}-lib
                      ▲                                     ▲
   *-component-skeleton/ (worked examples) ────────────────┘
                                                            │
                ggcommons-test-infra  ──exercises all 3 libs (broker, TLS, cross-lang interop)
```

- The **CLI** clones/copies a **template**, substitutes placeholders, and produces a runnable
  component project that depends on the language's **library**.
- The **skeletons** are fuller, hand-maintained examples of components built on the libraries.
- **test-infra** stands up the broker and runs each library's broker-backed integration tests
  plus the cross-language interop suite (a component in each language exchanging MQTT messages).

## Quick start

Build a new component with the CLI (see `ggcommons-cli/README.md` for the full reference):

```bash
pipx install ./ggcommons-cli                 # or: python -m pip install ./ggcommons-cli
ggcommons doctor                             # check prerequisites (git, gdk, cargo, mvn, python3, aws)
ggcommons create-component -n com.example.MyComponent -l PYTHON   # scaffold ./MyComponent
```

Run a component locally in STANDALONE mode against a local MQTT broker:

```bash
# bring up the shared broker (from ggcommons-test-infra)
docker compose -f ggcommons-test-infra/compose.yaml up -d
# then run a component (example shapes; see each skeleton's README)
python3 main.py -m STANDALONE standalone-messaging.json -c FILE config.json -t my-thing
java   -jar target/<artifact>.jar -m STANDALONE ./standalone-messaging.json -c FILE ./config.json -t my-thing
```

Components are packaged and deployed with the **GDK (Greengrass Development Kit)** —
`gdk component build` then `gdk component publish`, configured per component in
`gdk-config.json` and `recipe.yaml`.

## Cross-cutting subsystems (in every library)

- **config/** — five config-source managers (`FILE`, `ENV`, `GG_CONFIG`, `SHADOW`,
  `CONFIG_COMPONENT`), template-variable substitution (`{ComponentName}`, `{ThingName}`,
  custom tags) with sanitization, hot reload, multi-instance config, and JSON-schema validation.
- **messaging/** — one interface over two providers: Greengrass **IPC** and the STANDALONE
  **dual-MQTT** (local + IoT Core). Connections and subscriptions block until confirmed; supports
  request/reply with correlation, a per-subscription concurrency cap, and TLS (server-only or
  mutual). The on-wire message envelope is identical across languages.
- **metrics/** — pluggable targets: CloudWatch (EMF), cloudwatch-component, messaging, and local
  log. Selected by configuration.
- **heartbeat/** — periodic system metrics (CPU/memory/disk/threads/FDs) routed through the
  metric or messaging subsystem.
- **logging/** — console plus optional size-rotated file logging.
- **builders** — fluent construction (`GGCommonsBuilder`, `MessageBuilder`, `MetricBuilder`, …).

## Parity

The Java, Python, and Rust libraries are intentional mirrors: the same config schema, CLI flags,
subsystem boundaries, and message wire format. **When changing public behavior in one, check
whether the others need the matching change** to preserve parity. Java is the canonical
reference.

## Testing

- Each library has its own unit/integration suites (see its README). The libraries' broker-backed
  integration tests and the cross-language interop suite use the shared **`ggcommons-test-infra`**
  broker + certs.
- **Cross-language interop:** `ggcommons-test-infra/interop/` runs a request/reply and raw
  publish/ingest round-trip for every ordered pair of languages, proving the MQTT envelope and
  conventions interoperate.

## Working in this workspace

Because these are independent repos (each on its own default branch — e.g. some on `main`, some
on `master`, and `ggcommons-python-lib` on `major-rearch`), clone/commit per repo; there is no
top-level build. Start from the README in whichever subproject you're working in.
