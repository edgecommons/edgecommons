# GGCommons CLI

A scaffolding command-line tool for building **AWS IoT Greengrass v2** components on top of
the `ggcommons` libraries (Java, Python, Rust, TypeScript). It generates new component projects
from language templates and helps validate, build, publish, deploy, and upgrade them.

Once installed it provides two equivalent commands: **`ggcommons`** and **`ggcommons-cli`**.

## Installation

Requires **Python 3.8+**.

```bash
pipx install .            # recommended: a global `ggcommons` command, isolated
python -m pip install .   # or a plain install
python -m pip install -e .   # editable install, for developing the CLI itself
```

The component templates are **bundled into the wheel at build time** (from the
monorepo's `templates/`), so an installed CLI scaffolds **offline** — no repo
checkout or network needed. Override the template source per command with
`--template-url` (a git URL or a local directory). An editable install falls back
to the in-repo `templates/`.

## Usage at a glance

```bash
ggcommons --help              # list all commands
ggcommons --version           # print the CLI version
ggcommons <command> --help    # full options for one command
```

Conventions that apply to every command:

- Running `ggcommons` with **no command** prints the top-level help.
- An **unknown command** prints `Unknown command: <x>` and exits `2`.
- A command that **fails** prints `error: <message>` (no stack trace) and exits `1`.
- Commands are **auto-discovered** from `ggcommons_cli/commands/*.py` (see *Extending* below),
  so the set below is whatever ships in this package.

---

## Commands

### `create-component` — scaffold a new component

Generates a new component project from the template for the chosen language, substitutes
placeholders, and runs post-generation checks.

**Result:** a new directory `<path>/<ComponentName>` is created (where `ComponentName` is the
**last dot-segment** of the fully-qualified `--name`, e.g. `com.example.MyComponent` →
`MyComponent`). The template is fetched (git clone or local copy), every `<<TOKEN>>`
placeholder is substituted (component name, description, author, bucket, region, jar name,
Rust path dependency, …), files are renamed per the template manifest, the template's
`ggcommons-template.json` manifest is removed, and the CLI then **fails fast if any
`<<...>>` token remains** and **lints the generated `recipe.yaml`** (printing
`WARNING (recipe): …` for anything that would break `gdk component publish`). On success it
prints `Done. Component generated at: <dir>`.

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `-n`, `--name` | yes¹ | – | Fully-qualified component name (e.g. `com.example.MyComponent`). The generated directory and recipe `ComponentName` use the last segment. |
| `-l`, `--language` | yes¹ | – | One of `JAVA`, `PYTHON`, `RUST`, `TYPESCRIPT`. |
| `-i`, `--interactive` | no | off | Prompt for the inputs with a guided wizard (Enter accepts each `[default]`). **Auto-enabled when `-n/--name` is omitted on a terminal.** |
| `--platforms` | no | `GREENGRASS,HOST,KUBERNETES` (all) | Comma-separated target platforms. Controls which **platform-specific artifacts** the template emits — e.g. the Kubernetes `Dockerfile` + `k8s/` manifests are generated **only when `KUBERNETES` is included**. |
| `--dep-source` | no | `local` | How the component depends on the ggcommons library: `local` (a path/`file:` dependency on a monorepo checkout) or `registry` (the published artifact). `registry` skips the `--ggcommons-path` requirement. |
| `-d`, `--description` | no | `This is a Greengrass v2 component` | Short description embedded in the recipe. |
| `-p`, `--path` | no | `.` | Directory to create the component in (the project is created at `<path>/<ComponentName>`). |
| `-j`, `--jar` | no | the component name | Jar file name (Java only). |
| `-a`, `--author` | no | `Amazon Web Services` | Component author. |
| `-b`, `--bucket` | no | `greengrass-component-artifacts-us-east-1` | S3 bucket recorded in `gdk-config.json`. |
| `-r`, `--region` | no | `us-east-1` | AWS region recorded in `gdk-config.json`. |
| `-g`, `--ggcommons-path` | no | the in-repo `libs/rust` (or `libs/ts` for TypeScript) | **Rust/TypeScript with `--dep-source local` only** — absolute path to the ggcommons library; becomes the Cargo **path** dependency (Rust) or npm `file:` dependency (TypeScript). |
| `-u`, `--template-url` | no | the built-in source for the language | Override the template source: a **git URL** (cloned) **or** a **local directory** (copied). |
| `--template-ref` | no | the repo's default branch | Git branch or tag to clone (ignored when `--template-url` is a local directory). |
| `-f`, `--force` | no | off | Overwrite the target directory if it already exists and is non-empty. |

¹ Required in non-interactive use. In `--interactive` mode (or when `-n` is omitted on a terminal) they are prompted for.

**Examples**

```bash
ggcommons create-component -n com.example.MyComponent -l RUST
```
Creates `./MyComponent/` from the Rust template, wiring the Cargo dependency to the in-repo
`libs/rust`, with the default author/bucket/region. Result: a ready-to-build Rust Greengrass
component project.

```bash
ggcommons create-component -n com.example.MyComponent -l JAVA -j my-component
```
Creates `./MyComponent/` from the Java template; the build/recipe use `my-component` as the
jar name instead of the default (`MyComponent`).

```bash
ggcommons create-component -n com.example.MyComponent -l PYTHON \
  -p ./components -a "Jane Dev" -b my-artifacts-bucket -r us-west-2 --force
```
Creates `./components/MyComponent/` from the Python template, overwriting it if present, with
the given author and an S3 bucket/region of `my-artifacts-bucket` / `us-west-2` in
`gdk-config.json`.

```bash
ggcommons create-component -n com.example.MyComponent -l PYTHON \
  -u ./templates/python
```
Generates from a **local** template directory instead of cloning the default git source —
useful for a forked/offline template.

```bash
ggcommons create-component -i
```
Runs the **interactive wizard**: prompts for language, name, description, **target platform(s)**,
and dependency source. Selecting only `GREENGRASS`/`HOST` omits the Kubernetes `Dockerfile` +
`k8s/` manifests; including `KUBERNETES` emits them. The same gating is available
non-interactively, e.g. `--platforms GREENGRASS,KUBERNETES`.

---

### `list-templates` — show available languages

Takes no options.

**Result:** prints the languages the CLI can generate and the default template source (git
URL) for each, e.g.:

```
Available templates (override any source with --template-url):

  JAVA        git@…/java-component-template.git
  PYTHON      git@…/python-component-template.git
  RUST        git@…/rust-component-template.git
  TYPESCRIPT  git@…/typescript-component-template.git
```

```bash
ggcommons list-templates
```

---

### `validate` — check a recipe for publish-readiness

Lints a component's `recipe.yaml` for constructs that make `gdk component publish` fail.

**Result:** prints `OK: <recipe> has no known GDK-publish issues.` and exits `0` if clean.
Otherwise it lists each problem and exits `1`. It flags:
- the `{COMPONENT_NAME}` placeholder (GDK does not substitute it → publish is rejected),
- an artifact `Permissions:` block (`CreateComponentVersion` rejects it),
- any leftover `<<...>>` template placeholders.

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `-p`, `--path` | no | `.` | Path to the component project **or** directly to a `recipe.yaml`. |

```bash
ggcommons validate -p ./MyComponent
```
Lints `./MyComponent/recipe.yaml`. Result: confirmation that the recipe is publish-ready, or
a numbered list of issues to fix (with a non-zero exit so it can gate CI).

---

### `doctor` — check prerequisites

Takes no options.

**Result:** prints an `[ok]`/`[missing]` line for each external tool the build/publish flows
need, followed by a summary. The tools checked are: `git` (clone templates), `gdk`
(build/publish), `cargo` (Rust builds), `mvn` (Java builds), `python3` (Python builds), and
`aws` (publish/deploy). Found tools show their resolved path; missing ones explain what they're
needed for. (It reports status only — it does not exit non-zero on missing tools.)

```bash
ggcommons doctor
```

---

### `deploy` — build, publish, and (optionally) deploy with the GDK

Runs the Greengrass Development Kit against a component project, and optionally creates a
cloud deployment. Requires `gdk` on `PATH` (and AWS credentials for publish/deploy).

**Result depends on the flags:**
- *(no flags)* — runs `gdk component build`, then prints a hint that you can `--publish` or
  `--target`.
- `--publish` — build, then `gdk component publish` (uploads artifacts + creates a component
  version in the cloud).
- `--target <arn>` — implies `--publish`, then reads the component name/version from
  `gdk-config.json` and runs `aws greengrassv2 create-deployment` to deploy that exact version
  to the target thing or thing-group ARN. (The version must be concrete — a `NEXT_PATCH`
  placeholder is rejected so the deployed version is unambiguous.)

> Note: this command does cloud deployments. **On-device local deployments** are done with
> `greengrass-cli` on the core itself and are out of scope here.

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `-p`, `--path` | no | `.` | Path to the component project. |
| `--publish` | no | off | Publish the component to the cloud after building. |
| `-t`, `--target` | no | – | Deployment target ARN (thing or thing group). Implies `--publish` and creates a cloud deployment. |
| `-r`, `--region` | no | `us-east-1` | AWS region for the deployment. |

```bash
ggcommons deploy -p ./MyComponent                 # build only
ggcommons deploy -p ./MyComponent --publish       # build + publish a component version
ggcommons deploy -p ./MyComponent \
  -t arn:aws:iot:us-east-1:123456789012:thinggroup/edge-fleet -r us-east-1
```
The last form builds, publishes, and creates a Greengrass v2 deployment of `MyComponent` (at
the version in `gdk-config.json`) to the `edge-fleet` thing group.

---

### `upgrade` — bump the ggcommons dependency

Updates a generated component's dependency on the ggcommons library to a specific version,
editing whichever manifest is present.

**Result:** rewrites the ggcommons dependency version in `pom.xml` (Java `<artifactId>ggcommons</artifactId>`),
`requirements.txt` (the `greengrass-commons` pin), and/or `Cargo.toml` (the `ggcommons`
version dependency), then prints one line per file describing the change. A Rust **path**
dependency is left untouched (there's no version to bump). If no ggcommons dependency is found,
it prints `No ggcommons dependency found to upgrade.`

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `-p`, `--path` | no | `.` | Path to the component project. |
| `-t`, `--to` | **yes** | – | Target ggcommons version (e.g. `1.2.3`). |

```bash
ggcommons upgrade -p ./MyComponent --to 1.3.2
```
Pins the component's ggcommons dependency to `1.3.2` in whichever manifest it uses, and reports
what changed.

---

## Typical workflow

```bash
ggcommons doctor                                   # confirm the tools you need are installed
ggcommons create-component -n com.example.Foo -l PYTHON   # scaffold ./Foo
ggcommons validate -p ./Foo                        # confirm the recipe is publish-ready
ggcommons deploy   -p ./Foo --publish              # build + publish
ggcommons upgrade  -p ./Foo --to 1.3.2             # later: move to a newer ggcommons
```

## Extending the CLI

- **Add a command:** drop a `ggcommons_cli/commands/<name>.py` exposing a class that extends
  `CommandBase` with an `execute_command(self, args)` method and a `get_json_configuration`
  **classmethod** returning `{ "name", "description", "parameters": [...] }`. Each parameter
  declares `name`, `description`, `type` (use `"boolean"` for flags), and optionally `short`,
  `required`, `default`, and `enum`. The framework auto-registers it as a subcommand and builds
  its `argparse` flags from that schema.
- **Add a language/template:** templates are **manifest-driven** — a template repo ships a
  `ggcommons-template.json` declaring the placeholder `substitutions` and file `renames`, so adding
  a language needs a template (and an entry in `create-component`'s template sources), not new
  CLI logic.
- **Conditional (platform-gated) artifacts:** a manifest may add a `conditional` array —
  `[{"when": "platform:KUBERNETES", "paths": ["Dockerfile", "k8s"]}, ...]`. Each entry's paths are
  generated **only when its `when` flag is active**; the active flags are one `platform:<P>` per
  selected `--platforms` value plus `dep:<source>` from `--dep-source`. Unmet paths are removed,
  and any `substitutions`/`renames` that reference a removed path are skipped (so a file can appear
  in both `substitutions` and `conditional`). This is how a template ships k8s artifacts that only
  materialize when the user targets Kubernetes.

## Repository structure

```
.
├── pyproject.toml                  # packaging + console entry points (ggcommons / ggcommons-cli)
├── ggcommons_cli/
│   ├── cli.py                      # framework: arg parsing + command auto-discovery
│   ├── recipe_lint.py             # shared Greengrass-recipe linting (used by validate + create-component)
│   └── commands/                   # one auto-discovered command per file
│       ├── create_component.py
│       ├── list_templates.py
│       ├── validate.py
│       ├── doctor.py
│       ├── deploy.py
│       └── upgrade.py
└── scripts/                        # legacy wrapper scripts (optional once installed)
```

## Data flow

```
[User input] -> ggcommons (cli.py) -> CLIFramework parses argv + selects the command
             -> the command's execute_command(args) runs (scaffold / lint / gdk / aws / edit files)
             -> output + exit code back to the user
```

`cli.py` keeps the CLI framework (arg parsing, command discovery, error handling) separate from
each command's logic in `commands/`, which is where the actual work happens.
