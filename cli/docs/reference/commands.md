# Reference — commands

Every verb, argument, and flag. The surface is noun-first: a family, then what you do to it.

```
edgecommons [OPTIONS] <COMMAND>
```

## Global options

Accepted by every command.

| Flag | Meaning |
|---|---|
| `--json` | Emit machine-readable JSON instead of human output |
| `-q`, `--quiet` | Suppress non-essential output |
| `-v`, `--verbose` | Increase verbosity; repeatable (`-vv`) |
| `--no-color` | Never emit colored output |
| `--yes` | Never prompt; a missing required input becomes a usage error instead of a question |
| `-h`, `--help` | Print help |
| `-V`, `--version` | Print version |

`--quiet` and `--verbose` are mutually exclusive.

## `component`

The component lifecycle: scaffold, validate, upgrade, version, package, release.

### `component new`

Scaffold a new component from the templates carried inside the binary.

```
edgecommons component new [OPTIONS]
```

**Identity and shape**

| Flag | Meaning |
|---|---|
| `-n`, `--name <NAME>` | Fully-qualified component name, e.g. `com.example.MyComponent` |
| `-l`, `--language <LANGUAGE>` | `JAVA`, `PYTHON`, `RUST`, `TYPESCRIPT` |
| `-k`, `--kind <KIND>` | `service` (default), `protocol-adapter`, `processor`, `sink` |
| `-d`, `--description <DESCRIPTION>` | Short description |
| `-a`, `--author <AUTHOR>` | Component author |
| `--license <LICENSE>` | `none` (default), `busl-1-1`, `apache-2-0`, `mit`. Writes a `LICENSE` file with the chosen SPDX text; `none` writes none |

**Where it lands**

| Flag | Meaning |
|---|---|
| `-p`, `--path <PATH>` | Parent directory the derived `<kebab-name>` output dir is created under. Default `.` |
| `--dir <DIR>` | Exact output directory; overrides the derived `<path>/<kebab-name>` outright |
| `--bin-name <BIN_NAME>` | Override the derived crate/binary name (kebab, `^[a-z0-9][a-z0-9-]*$`). Also names the default output dir when `--dir` is absent. Alias: `--crate-name` |
| `-f`, `--force` | Overwrite a non-empty target directory |

**Platforms and packaging**

| Flag | Meaning |
|---|---|
| `--platforms <PLATFORMS>` | `GREENGRASS`, `HOST`, `KUBERNETES` — controls which artifact packs are emitted |
| `-b`, `--bucket <BUCKET>` | S3 bucket for Greengrass component artifacts. Only used when the GREENGRASS pack is emitted |
| `-r`, `--region <REGION>` | AWS region for Greengrass publishing. Default `us-east-1`. GREENGRASS pack only |

**How it depends on the library**

| Flag | Meaning |
|---|---|
| `--dep-source <DEP_SOURCE>` | `local` (default), `registry`, or `pinned-rev` — a git dependency pinned to an exact revision plus a gitignored local-dev override (Rust/Python only) |
| `--library-path <LIBRARY_PATH>` | Path to a local library checkout — for `--dep-source local`, and the `.cargo` local-dev override under `pinned-rev` |
| `--library-rev <LIBRARY_REV>` | Git revision to pin the library to (for `pinned-rev`). Defaults to the commit this CLI was built from |

**Template source**

| Flag | Meaning |
|---|---|
| `--template-dir <TEMPLATE_DIR>` | Use a template from a local directory instead of the embedded one |
| `--template-git <TEMPLATE_GIT>` | Clone a template from a git URL. **The only network access `component new` ever makes** |

The output directory name is derived from `--name` in kebab form unless `--dir` or `--bin-name` says
otherwise. Missing required inputs are prompted for interactively unless `--yes` is set, which turns
them into usage errors instead.

### `component validate`

Validate a component's config and artifacts.

```
edgecommons component validate [OPTIONS]
```

| Flag | Meaning |
|---|---|
| `-p`, `--path <PATH>` | The component project directory. Default `.` |
| `-c`, `--config <CONFIG>` | Validate this config file specifically. Default: every config the project ships |
| `--platform <PLATFORM>` | `GREENGRASS`, `HOST`, `KUBERNETES` — the platform this config is destined for |

`--platform` changes coverage, not just messages. Rules that are only decidable with a platform in
hand — a transport or config source legal on one platform and illegal on another — are **skipped when
it is absent**, rather than guessed at.

Validation runs in three layers: schema (`EC1xxx`), semantic rules (`EC2xxx`), and artifact lint
(`EC3xxx`). Findings exit `1`; a clean run exits `0`.

### `component upgrade`

Move the component to a given **edgecommons library** version. (For the component's own version, see
`component version`.)

```
edgecommons component upgrade [OPTIONS]
```

| Flag | Meaning |
|---|---|
| `-p`, `--path <PATH>` | Project directory. Default `.` |
| `-t`, `--to <TO>` | Target library **version**; rewrites a git-rev pin to the release-tag form |
| `--to-rev <TO_REV>` | Move the library **git rev** pin to this revision (Rust/Python only) |
| `--dry-run` | Show what would change without writing |

`--to` and `--to-rev` are mutually exclusive.

### `component version`

Set the **component's own** version across its manifests.

```
edgecommons component version [OPTIONS] --to <TO>
```

| Flag | Meaning |
|---|---|
| `-p`, `--path <PATH>` | Project directory. Default `.` |
| `-t`, `--to <TO>` | The component's new version (required) |
| `--dry-run` | Show what would change without writing |

The stated version is authoritative — there is no semver-inference from commit history.

### `component package`

Build deployable artifacts for the selected platform(s).

```
edgecommons component package [OPTIONS]
```

| Flag | Meaning |
|---|---|
| `-p`, `--path <PATH>` | Project directory. Default `.` |
| `--platforms <PLATFORMS>` | `GREENGRASS`, `HOST`, `KUBERNETES` |
| `--publish` | Publish the built artifact (Greengrass: `gdk component publish`) |

Container images are built by CI, not by this verb.

### `component release`

Build artifacts, compute digests, and emit a release descriptor.

```
edgecommons component release [OPTIONS]
```

| Flag | Meaning |
|---|---|
| `-p`, `--path <PATH>` | Project directory. Default `.` |
| `-o`, `--out <OUT>` | Where to write the release descriptor. Default `release.json` |

This verb **never tags, uploads, or publishes**. The CLI produces; the runner publishes.

## `template`

Inspect the component templates this binary can generate.

```
edgecommons template list            # the language × kind matrix
edgecommons template show <ID>       # one template's manifest
```

`<ID>` is `<language>/<kind>` in lowercase, e.g. `rust/service`. `show` prints the supported
platforms, the substitution tokens, and every file the template emits. An unknown id is `EC4003`.

## `registry`

Query the EdgeCommons ecosystem catalog.

```
edgecommons registry list [OPTIONS]
edgecommons registry show <NAME> [OPTIONS]
edgecommons registry versions <NAME> [OPTIONS]
```

| Flag | Meaning |
|---|---|
| `--source <SOURCE>` | Registry URL or a local `components.json` path. Env: `EDGECOMMONS_REGISTRY_URL` |
| `--language <LANGUAGE>` | Filter: `JAVA`, `PYTHON`, `RUST`, `TYPESCRIPT` (`list` only) |
| `--category <CATEGORY>` | Filter by catalog category (`list` only) |

Categories are `adapter`, `processor`, `sink`, `bridge`, `console`, `service`, and `tool`. A `tool` is
an operator or developer CLI built on the library — run from a shell, not deployed to a device.

## `deployment`

Compile a deployment definition into platform-native artifacts. `validate`, `render`, and `plan` run
with **no server and no network**.

```
edgecommons deployment validate <DEFINITION>
edgecommons deployment lock     <DEFINITION>
edgecommons deployment render   <DEFINITION> --env <ENV> --target <TARGET>
edgecommons deployment plan     <DEFINITION> --env <ENV> --target <TARGET>
edgecommons deployment diff     <DEFINITION> --against <REF>
edgecommons deployment release  <DEFINITION> --stream <STREAM>
```

| Verb | In | Out |
|---|---|---|
| `validate` | a definition | Three stages: the definition's own schema, the semantic rules (S-1..S-9), then every rendered effective config against the strict runtime schema |
| `lock` | definition + release index | Resolves each pinned version to an immutable digest. **The only verb that touches the network** |
| `render` | definition, env, target | Native artifacts for the target plus the normalized plan, written under `render/<target>/`. Nothing is committed |
| `plan` | definition, env, target | The normalized plan JSON alone — the common currency for validation, policy, CI, and the UI |
| `diff` | a Git ref | The delta grouped by consequence: restart, storage, network, identity, permission, config, artifact, apply-order |
| `release` | definition + the stream being promoted | Promotes **one** stream and writes the release manifest and lock |

Options: `--env <ENV>` and `--target <TARGET>` (`GREENGRASS`, `HOST`, `KUBERNETES`) for `render` and
`plan`; `--against <REF>` for `diff`; `--stream <STREAM>` (`config` or `artifact`) for `release`.

**Targets.** The definition's `targetStandard.family` must match `--target`; a mismatch is a usage
error rather than a silent retarget. The HOST and Greengrass renderers are built; Kubernetes is not
yet, and says so (exit `5`).

**Greengrass** renders **one deployment document per thing** — thing ARNs, never thing groups — so a
definition's nodes map one-to-one onto deployments and failure is per node. Components taking
`GG_CONFIG` carry their effective config as a `configurationUpdate` merge. Recipes are *not* produced
here: a recipe is a packaging artifact of a component release.

**Streams.** Config and artifact are independently versioned and independently reconciled. The
release lock correlates them without fusing them, so either rolls back alone.

`lock` and `diff` are declared but not built in this binary; they exit `5`.

## `studio`

```
edgecommons studio serve [--repo <REPO>] [--bind <BIND>]
```

The Deployment Studio server over the same kernel the CLI uses. `--repo` defaults to `.`, `--bind` to
`127.0.0.1:8787`. The subcommand exists and the ports are wired; the server is not built in this
binary and exits `5`.

## `doctor`

Check the external tools your targets need.

```
edgecommons doctor [OPTIONS]
```

| Flag | Meaning |
|---|---|
| `--platforms <PLATFORMS>` | Only check what these platforms need. Defaults to all |
| `-l`, `--language <LANGUAGE>` | Only check what this language needs. Defaults to all |

Reports missing (`EC0001`) and too-old (`EC0002`) tools. It never installs anything.

## `completions`

```
edgecommons completions <SHELL>
```

`<SHELL>` is `bash`, `zsh`, `fish`, `powershell`, or `elvish`. Writes the script to stdout.
