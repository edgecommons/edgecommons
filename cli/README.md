# `edgecommons` — the EdgeCommons CLI

A single static binary that scaffolds, validates, and releases EdgeCommons components.

Scaffolding a Java or TypeScript component needs no Python runtime and no JVM — just this binary.

## Install

```bash
cargo install --path cli/crates/ec-cli
```

Or build it in place:

```bash
cd cli && cargo build --release   # -> cli/target/release/edgecommons
```

Templates and the canonical config schema are compiled into the binary, so scaffolding and
validation work offline.

## Commands

| Command | What it does |
|---|---|
| `component new` | Scaffold a component: language × kind × platforms. |
| `component validate` | Check a component's config and artifacts. |
| `component upgrade` | Move a component to a given **edgecommons library** version. |
| `component version` | Set the **component's own** version across its manifests. |
| `component package` | Build deployable artifacts for the selected platforms. |
| `component release` | Build, digest, and emit a release descriptor. Never publishes. |
| `template list` / `show` | The language × kind matrix, and one template's contents. |
| `registry list` / `show` / `versions` | The ecosystem catalog. |
| `deployment …` | Model-to-artifact deployment. Not available in this build. |
| `studio serve` | The Deployment Studio server. Not available in this build. |
| `doctor` | Check the external tools the platforms you target need. |
| `completions <shell>` | A shell completion script. |

Every command takes `--json`.

### Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | The command ran and found problems (validation failed, lint errors). |
| `2` | The command was invoked incorrectly. |
| `3` | A required external tool is missing. |
| `4` | An internal error. |
| `5` | The verb is declared but not available in this build. |

## Scaffolding

```bash
edgecommons template list
edgecommons component new -n com.example.MyComponent -l RUST
edgecommons component new -n com.example.MyAdapter -l PYTHON -k protocol-adapter
```

Kinds are `service`, `protocol-adapter`, `processor`, and `sink`.

`--platforms` selects which artifacts are emitted. A HOST-only component gets a `compose.yaml`
and a supervisord program block and no Greengrass recipe; a Kubernetes one gets a `Dockerfile`
and `k8s/` manifests:

```bash
edgecommons component new -n com.example.Thing -l RUST --platforms HOST
```

Every scaffold ships a `config.schema.json` describing its own configuration — what goes under
`component.global`. That is the file `component validate` checks a config against, and it
travels with the component's release descriptor.

## Validating

```bash
edgecommons component validate -p MyComponent
```

Three layers, one diagnostic stream:

* **`EC1xxx`** — the canonical EdgeCommons config schema, plus the component's own
  `config.schema.json`.
* **`EC2xxx`** — semantic rules JSON Schema cannot express: `IPC` only on `GREENGRASS`,
  `CONFIGMAP` only on `KUBERNETES`, no secret values, no `CONFIG_COMPONENT` bootstrap loop.
* **`EC3xxx`** — the Greengrass recipe and `gdk-config.json`, parsed.

```text
error[EC1002] MyComponent/test-configs/config.json:/component/global
  Additional properties are not allowed ('publish_intervall' was unexpected)
  help: this key is not accepted by the component's own config.schema.json
```

## Versions

`component upgrade` moves the **edgecommons library** dependency. `component version` sets the
**component's own** version. They are different things:

```bash
edgecommons component upgrade -p MyComponent --to 0.3.0   # the library
edgecommons component version -p MyComponent --to 1.4.2   # the component
```

## Releasing

`component release` builds the artifacts, computes their digests, and writes a release
descriptor. **It publishes nothing.** Tagging, uploading, and cataloguing belong to a release
workflow running this same binary in CI — so a local run emits exactly the bytes CI would, which
is what makes the descriptor reviewable before it is real.

```bash
edgecommons component version -p MyComponent --to 1.4.2
edgecommons component release -p MyComponent --out release.json
```

## Layout

| Crate | Responsibility |
|---|---|
| `ec-cli` | The binary: argument parsing, output, exit codes. |
| `ec-diag` | The diagnostic model and its human/JSON renderers. |
| `ec-scaffold` | Embedded templates, the manifest engine, generation, version manipulation. |
| `ec-validate` | The canonical schema, the component schema, semantic rules, artifact lint. |
| `ec-deploy` | The deployment kernel: model, plan, and the five ports. No I/O. |
| `ec-adapters` | Adapters behind the ports. The only crate that may link a cloud SDK. |
| `ec-studio` | The Deployment Studio server shell. |

Design: [`docs/platform/DESIGN-cli.md`](../docs/platform/DESIGN-cli.md).
