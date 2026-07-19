# Design — the `edgecommons` CLI (Rust, greenfield)

> Companion to [DESIGN-core.md](DESIGN-core.md) / [DESIGN-packaging.md](DESIGN-packaging.md).
> **Status: PROPOSED.** A greenfield Rust replacement for the Python `cli/`. Defines the whole binary:
> the component surface (scaffold, validate, upgrade, package), the template model, the validation
> engine, the registry surface, `doctor`, and — at **contract level** — the deployment verbs and the
> five ports that make the binary the Deployment Studio kernel. Renderer internals stay in the
> Deployment Studio design deck.
>
> **Supersedes** `cli/` (Python, 7 commands, ~1,300 LOC). That tree is deleted at the end of Phase 3.
>
> **Sources (re-read, not recalled):** `roadmap/ROADMAP.md` **RM-012** (Accepted) and RM-002;
> the Deployment Studio deck ch. 12 and `REVIEW.md` decisions #1/#9/#2/#3; the CLI feature/requirements
> review of 2026-07-11; and the current `cli/` + `templates/` sources.

---

## 1. Why greenfield, and not a port

RM-012 already decided the *language*: the deployment kernel must be Rust, so the binary exists either
way, and keeping a Python CLI in front of it would force users to install two runtimes plus a
trampoline. This document takes the next step and decides the *shape*, because a faithful port would
carry three things forward that we do not want:

1. **A Greengrass-era product.** The CLI's defaults, prerequisites, artifacts, and its only deploy path
   all assume Greengrass. Platform neutrality (PN-1) is the differentiator; the front door contradicts it.
2. **A reflective plugin framework.** `cli.py` discovers command classes with `importlib` +
   `inspect.getmembers`, validates a JSON descriptor per command with `jsonschema`, and swallows
   registration failures through `warnings.warn` — 160 lines of meta-machinery to register seven static
   commands. `clap`'s derive API is the whole of it in Rust.
3. **Ten shipped defects.** RM-012 names the existing pytest suite as "the oracle for the rewrite". It
   cannot be: the suite is green while the defects in §12 are live, because the code paths that are
   broken are exactly the ones with no tests. **The oracle is this specification plus the
   scaffold→build→run gate (§10), and the defect register in §12 becomes regression tests.**

### 1.1 What survives the rewrite

Two properties are load-bearing and non-negotiable (RM-012 states both):

- **Templates are embedded in the binary, so scaffolding works offline.** Today `setup.py` copies
  `templates/` into the wheel at build time; the Rust binary embeds them at compile time.
- **The interactive wizard and conditional (platform-gated) artifact generation are ported as
  *behavior*, not redesigned.** They are the most-tested part of the current CLI and they work.

One idea is promoted rather than preserved: **the template manifest**. The deck asks us to "reuse the
template manifest idea for deployment renderers and output packs" — so §5 generalizes it into a single
*output pack* concept that spans scaffolding templates and, later, deployment renderers.

---

## 2. Scope

**In scope.** The single static binary: `component`, `template`, `registry`, `doctor`, `completions`;
the `deployment` verbs' command contract, the hexagonal core, and the five ports as compiling seams; the
`studio serve` seam.

**Out of scope.** Renderer internals (HOST supervisord / Kubernetes / Greengrass output packs), the plan
JSON's full field list, the Studio UI, and release/evidence governance — all owned by the Deployment
Studio deck. This document defines *where they plug in* and *what the CLI guarantees them*, so the two
can be built independently and meet at the port boundary.

---

## 3. Principles

| # | Principle | Consequence |
|---|---|---|
| P1 | **One static binary, no daemon.** | `component`, `deployment`, and `studio serve` are subcommands of one artifact. Scaffolding a Java component needs no Python and no JVM. |
| P2 | **Offline by construction.** | Templates and the canonical config schema are compiled in. `component new` and `deployment validate\|render\|plan\|diff` never touch the network. A CLI plus a Git bundle on removable media is a complete workflow. |
| P3 | **No cloud SDK above the port boundary.** | The Greengrass adapter may link the AWS SDK; nothing else may. Today's CLI already satisfies this by shelling out to `gdk`/`aws`; keep it that way. |
| P4 | **Call `layered.rs`; never reimplement the merge.** | Effective config is computed by `core/libs/rust`'s `config/layered.rs` — the same code the runtime executes — so determinism is *structural*, not asserted. Same for `uns.rs` topic validation. |
| P5 | **Determinism is a build gate, not a claim.** | Byte-stable serialization rules (§8.3) with golden-file suites. Every render is reproducible from (definition commit, renderer version, inputs). |
| P6 | **Machine-readable by default.** | Every verb supports `--json`. One diagnostic model (§6.4) drives both human and JSON output. The plan JSON is the common currency between CLI, CI, policy, and UI. |
| P7 | **Not bound by four-language parity.** | The CLI is a tool, not the core library (`config-component` is the precedent). Parity is triggered only if a change reaches the wire or the config schema. |

---

## 4. Command surface

The current surface is flat and inconsistent (`create-component` vs `list-components`), and it collides
with the Studio: `edgecommons validate` (recipe lint) versus `edgecommons deployment validate` (config
schema) is a trap. The new surface is **noun–verb**. The rename is free exactly once — the CLI is
unpublished, installed with `pipx install .` from a checkout, with no user base to break (RM-012). We
take that window now; there are **no aliases** to the old names.

```
edgecommons
├── component
│   ├── new           scaffold a component (language × kind × platforms)
│   ├── validate      config schema + semantic rules + artifact lint
│   ├── upgrade       move a component to a given *edgecommons* version
│   ├── version       set the *component's own* version across its manifests
│   ├── package       build deployable artifacts for the selected platform(s)
│   └── release       build + digest + emit the release descriptor (never publishes — §7.3)
├── template
│   ├── list          the language × kind matrix, from the embedded manifests
│   └── show <id>     one template's manifest: platforms, tokens, emitted files
├── registry
│   ├── list          the ecosystem catalog (filters, --json)
│   ├── show <name>   one catalog entry
│   └── versions <n>  the published releases of a component, from the release index (§9.1)
├── deployment        ── contract in §8; hexagonal core behind it
│   ├── validate      deployment model + every rendered effective config
│   ├── lock          resolve pinned versions → digests; the one networked verb (§8.7)
│   ├── render        model → native artifacts + the normalized plan
│   ├── plan          the normalized plan JSON alone
│   ├── diff          this render vs a release ref, grouped by consequence
│   └── release       promote a stream: --stream artifact|config (§8.5)
├── studio
│   └── serve         the server shell around the same kernel (seam only in v1)
├── doctor            platform-aware prerequisite check
└── completions <shell>
```

### 4.1 Migration from the current surface

| Today | Becomes | Note |
|---|---|---|
| `create-component` | `component new` | Wizard + conditional generation preserved (§5.4). |
| `validate` | `component validate` | **Massively widened** — today it is recipe lint only (§6). |
| `upgrade` | `component upgrade` | All four dependency forms actually work (§7). |
| `deploy` (gdk build/publish) | `component package [--publish]` | The build/publish half. |
| `deploy --target <arn>` | **removed in v1** | The apply half. See the deviation in §8.6. |
| `list-templates` | `template list` | Now manifest-driven, not a hardcoded dict. |
| `list-components` | `registry list` | Behavior preserved, filters corrected (§9). |
| `doctor` | `doctor` | Platform-aware, non-zero exit, version checks (§9). |
| *(none)* | `component validate`, `component version`, `component release`, `template show`, `registry show\|versions`, `deployment *`, `studio serve`, `completions` | New. |
| `edgecommons add <name>` | **never existed** | Promised twice in `docs/ECOSYSTEM.md`; delete the promise (§12, DEF-11). |

### 4.2 Global conventions

- **Global flags:** `--json`, `-q/--quiet`, `-v/--verbose` (repeatable), `--no-color`, `--yes`
  (non-interactive; never prompt), `--version`.
- **Exit codes:** `0` success · `1` findings (validation failed, lint errors) · `2` usage error ·
  `3` environment error (a required external tool is missing) · `4` internal error · `5` the verb is
  declared but **not implemented in this build**. `doctor` returns `3` when a prerequisite for a *selected*
  platform is missing — today it always returns `0`.

  `5` exists because the surface is declared in full from Phase P0 while the verbs land across P1–P4
  (§11.3). Without it, an unbuilt verb has to masquerade as a usage error or an internal crash, and CI
  cannot distinguish *"this build cannot do that yet"* from *"you invoked it wrong."* It disappears from
  the surface as the phases complete.
- **TTY behavior:** interactive prompting is auto-enabled when required inputs are absent **and** stdin
  is a TTY **and** `--yes` is absent. Otherwise a missing required input is a usage error (`2`). This
  preserves today's rule (a wizard when `-n` is omitted on a terminal) while making CI failure explicit.
- **No network in `component new`.** `--template-git <url>` is the sole opt-in exception.

---

## 5. The template model (manifest v2)

### 5.1 Two axes, not one

Template selection today is one-dimensional (language) and hardcoded in a Python dict. The ecosystem is
two-dimensional. Evidence: `templates/java-protocol-adapter/` and `templates/python-protocol-adapter/`
already exist, complete with manifests, and are **unreachable** from the CLI; the roadmap wants a
`processor` scaffold; the camera-adapter work needed a Rust protocol-adapter.

A template is identified by **`<language>/<kind>`**:

| | `service` | `protocol-adapter` | `processor` | `sink` |
|---|---|---|---|---|
| **java** | ✅ exists | ✅ exists (unreachable today) | — | — |
| **python** | ✅ exists | ✅ exists (unreachable today) | — | — |
| **rust** | ✅ exists | — | — | — |
| **typescript** | ✅ exists | — | — | — |

`service` is the plain baseline (today's four templates). The other three mirror the registry's own
category vocabulary (`adapter`, `processor`, `sink`), so a scaffolded component and its catalog entry
speak the same word. Empty cells are not CLI work — they are template work, and adding one requires
**no CLI change**, which is the manifest-driven promise the current code makes and does not keep.

### 5.2 Discovery

Templates are discovered by scanning the embedded template tree and reading each manifest. There is no
registry of templates in code. `template list` renders what it finds; `component new` resolves
`--language`/`--kind` against it. A template that ships without a valid manifest is a **build failure**
of the CLI, not a runtime warning (today a bad command class merely warns and disappears).

### 5.3 Manifest schema

```jsonc
{
  "schemaVersion": 2,
  "id": "rust/service",              // must equal <language>/<kind> and the directory path
  "language": "RUST",                // JAVA | PYTHON | RUST | TYPESCRIPT
  "kind": "service",                 // service | protocol-adapter | processor | sink
  "description": "Rust component built on the edgecommons Rust library.",
  "platforms": ["GREENGRASS", "HOST", "KUBERNETES"],   // what this template can emit
  "requires": ["EDGECOMMONS_DEP"],   // tokens that must resolve non-empty
  "substitutions": {                 // file -> tokens; replaces <<TOKEN>>
    "Cargo.toml": ["BINNAME", "DESCRIPTION", "EDGECOMMONS_DEP"]
  },
  "renames": [                       // {TOKEN} interpolation in paths
    { "from": "src/main/java/com/mbreissi/testcomponent", "to": "src/main/java/{PACKAGEPATH}" }
  ],
  "packs": {                         // NEW: platform-gated artifact groups (see 5.5)
    "GREENGRASS": ["recipe.yaml", "gdk-config.json"],
    "HOST":       ["compose.yaml", "supervisor/component.conf"],
    "KUBERNETES": ["Dockerfile", "k8s/"]
  },
  "conditional": [                   // retained: arbitrary flag-gated paths
    { "when": "dep:registry", "paths": [".npmrc"] }
  ]
}
```

Flag namespaces for `conditional`: `platform:<P>`, `dep:<local|registry>`, `kind:<K>`. The manifest is
itself validated against an embedded JSON Schema at CLI build time, so manifest drift cannot ship.

### 5.4 Generation pipeline

Behavior-preserving with respect to today's `_apply_manifest`, in this order: resolve inputs (wizard or
flags) → copy the embedded tree → prune packs and unmet conditionals → substitute `<<TOKEN>>` → apply
renames → prune empty dirs → **verify no `<<...>>` survives** (a hard error today; keep it) → run the
artifact lint from §6.3 over what was emitted.

### 5.5 Platform packs fix a real asymmetry

Today only Kubernetes artifacts are platform-gated. Greengrass artifacts (`recipe.yaml`,
`gdk-config.json`) are emitted **unconditionally** — a HOST-only scaffold still gets a Greengrass recipe —
and **HOST, a first-class platform, gets no artifacts at all**. Packs make all three symmetric, and the
HOST pack (compose + a supervisord program block) is deliberately shaped to match the Studio's HOST
renderer output, so a hand-scaffolded component and a Studio-rendered one agree.

### 5.6 Every template ships a `config.schema.json`

A scaffolded component **declares the shape of its own config from day one**. The template emits a
`config.schema.json` (seeded with the component's example config) plus the wiring that hands it to the
library, because that one artifact is consumed in three places:

| Consumer | When | Effect |
|---|---|---|
| `component validate` (§6.1) | Authoring | The author's own config is actually checked — today nothing checks it. |
| `deployment validate` (§8.5.5) | Deploy | The effective config is checked against the schema of the **pinned version**, which is what makes the compatibility guard exact. |
| The **runtime** (RM-014) | Startup + hot reload | Fail fast at startup; reject-and-keep-last-good on reload. |

This is why the schema is a first-class scaffold artifact rather than an optional extra: it is the single
thing that closes the component-config validation hole at all three stages at once. A component that opts
out simply ships no schema, and every stage says so rather than implying coverage it does not have.

### 5.7 Token set

`COMPONENTFULLNAME`, `COMPONENTNAME`, `PACKAGE`, `PACKAGEPATH`, `MAINCLASSNAME`, `JARNAME`, `BINNAME`,
`SNAKENAME`, `LICENSE`, `LIBRARY_LOCAL_PATH`, `DESCRIPTION`, `AUTHOR`, `EDGECOMMONS_VERSION`,
`EDGECOMMONS_DEP`, plus the Greengrass-only `BUCKET` and
`REGION`, which are **prompted and substituted only when the GREENGRASS pack is selected** (`-b/--bucket`,
`-r/--region`).

**`BINNAME` is kebab-case, derived by case-boundary + acronym-aware splitting** of the short name
(`EthernetIpAdapter → ethernet-ip-adapter`, `OPCUAAdapter → opcua-adapter`), overridable with
`--bin-name` (alias `--crate-name`); it is the single source every consumer flows through
(crate/`[[bin]]` name, Maven artifactId/finalName = `JARNAME`, the Python module dir = `SNAKENAME`,
the npm `name`, Dockerfile/recipe/supervisor/compose/k8s names, test-configs). The **Greengrass
component name** (`COMPONENTFULLNAME`) stays PascalCase reverse-DNS — only the crate/bin/artifact/dir
tokens are kebab (D-CLI-17). The default **output directory** is `path/<BINNAME>`; `--dir` overrides it
outright. `LICENSE` is the SPDX id chosen by `--license` (default `none` — no LICENSE file, no manifest
license claim; the scaffold is the author's component, D-CLI-21). `LIBRARY_LOCAL_PATH` is the local
sibling checkout path emitted into the `dep:pinned-rev`-gated `.cargo/config.toml` override (D-CLI-18).

The AWS-era defaults (`author = "Amazon Web Services"`,
`bucket = "greengrass-component-artifacts-us-east-1"`, `description = "This is a Greengrass v2
component"`) are deleted — the old CLI asked every author for an S3 bucket regardless of what they were
building.

A Greengrass scaffold generated **without** a bucket cannot publish as it stands, so its absence is
reported (`EC4005`) **and** the visible sentinel `edgecommons-set-artifact-bucket` is substituted for
`BUCKET` (rather than an empty string that reads as intentional); `component validate` then **errors**
(`EC3007`) on the sentinel, so the miss is caught at authoring/CI, not at `gdk component publish` weeks
later (DEF-16, D-CLI-20).

**`EDGECOMMONS_VERSION` has exactly one source of truth**: it is resolved at CLI build time from the
workspace (`libs/rust/Cargo.toml`), never hand-maintained in a constant. That constant is how the
current CLI emits a Cargo dependency on the git tag `rust-lib/v0.1.0`, **which does not exist** (DEF-1).

---

## 6. The validation engine

This is the largest new capability, and the one both audiences want. Today `edgecommons validate` is a
regex pass over recipe *text* — the YAML is never parsed — and the CLI **cannot validate a component
config against the canonical schema at all**, even though `jsonschema` is a declared dependency that no
command uses. The Studio requires exactly this capability ("validate the rendered effective config, not
only the deployment source shape"), so it is built once, here, and reused by `deployment validate`.

Three layers, run in order, all feeding one diagnostic stream:

### 6.1 Layer 1 — schema

Two schemas, because one of them does not exist yet and its absence is a live hole.

**(a) The library envelope.** The canonical `schema/edgecommons-config-schema.json` is **embedded at compile
time** (offline, P2) and enforced with a Rust JSON Schema validator. Drift between the embedded copy and
`schema/` is a CI gate, exactly as the existing `sync-schema.sh --check` gate works for the four libraries.

**(b) The component's own config — unvalidated today.** The canonical schema is strict at the top level
(`additionalProperties: false`), but **`component.global` is `additionalProperties: true` with zero declared
properties**, and **no component repo ships a config schema**. So a component's own configuration — the part
an author most often gets wrong — is validated by *nothing*, at any stage, today.

The fix is a **per-component, per-version config schema**, published as a release artifact in the RM-013
release descriptor. `component validate` uses the schema in the component's own repo; `deployment validate`
uses the schema published by the **exact version being deployed**, which is also what makes the two-stream
compatibility guard exact rather than a declared version floor (§8.5.5). Where a component publishes no
schema, both verbs **warn and say so** rather than implying coverage they do not have.

### 6.2 Layer 2 — semantic rules

The rules JSON Schema cannot express. Each has a stable code and a `--fix` hint where one exists:

| Code | Rule | Where |
|---|---|---|
| `EC2001` | `--transport IPC` is valid only on `--platform GREENGRASS`. | component + deployment |
| `EC2002` | A supervisord/HOST render requires `--platform HOST`. | **deployment only** — it is a property of a *render*, not of a component's config, so `component validate` has nothing to check it against. |
| `EC2003` | A Kubernetes ConfigMap mount must not use `subPath`. | component (k8s pack) + deployment |
| `EC2004` | A hierarchical config lineage must be acyclic and ordered. | component + deployment |
| `EC2005` | Secret **values** are forbidden anywhere in a definition or config; only `secret://` references. | component + deployment |
| `EC2006` | A raw publish to a reserved UNS class (`state`, `metric`, `cfg`, `log`) is rejected. | component + deployment |
| `EC2007` | A component bootstrapping from `CONFIG_COMPONENT` may not depend recursively on `CONFIG_COMPONENT` for its own bootstrap config. | component + deployment |
| `EC2008` | UNS identity tokens must satisfy the char-set and the IoT-Core depth guard. | component + deployment |
| `EC2009` | A component's config source must be legal for its platform — `CONFIGMAP` only on KUBERNETES, `GG_CONFIG` only on GREENGRASS (§8.5.3). | component + deployment |

**`EC2001` and `EC2009` are only decidable with a platform** — the same transport or config source
is legal on one and illegal on another. `component validate` therefore takes **`--platform`**;
without it those two rules are *skipped*, not guessed at. A validator that invents a verdict it
cannot justify is worse than one that stays quiet, and says so.

### 6.3 Layer 3 — artifact lint

Emitted/on-disk artifacts, **parsed, not regexed**:

- **Greengrass recipe** (YAML-parsed): the three existing hard checks (`{COMPONENT_NAME}` placeholder,
  artifact `Permissions:` block, unsubstituted `<<...>>`) **plus `RequiresPrivilege: true`**, which
  exists today as `lint_least_privilege` but was never wired into `validate` (DEF-9).
- **`gdk-config.json`**: parsed and checked — not looked at at all today.
- **Kubernetes manifests**: parsed; `subPath` guard (`EC2003`).
- **HOST**: supervisord INI section uniqueness.

### 6.4 One diagnostic model

```rust
pub struct Diagnostic {
    pub code: Code,          // EC1xxx schema · EC2xxx semantic · EC3xxx artifact · EC4xxx template · EC5xxx deployment
    pub severity: Severity,  // Error | Warning
    pub file: Option<PathBuf>,
    pub locus: Option<Locus>,// line/col, or a JSON Pointer into the config
    pub message: String,
    pub help: Option<String>,
}
```

Rendered human-readable by default and as a stable array under `--json`. `component validate` and
`deployment validate` differ only in *what they collect*, never in how they report. Warnings do not
change the exit code; errors yield `1`.

---

## 7. Dependency, version, and release

### 7.1 `component upgrade`, done correctly

Today `upgrade` is a set of regexes that are wrong for three of the four languages: it silently no-ops on
every TypeScript component (the template emits the scoped key `@edgecommons/edgecommons`; the regex looks
for the bare key), it **corrupts** Python components (rewriting the real `edgecommons @ git+https://…`
form into `edgecommons==X`), and it cannot bump the git-tag Cargo dependency that the CLI itself emits.

The rewrite manipulates each manifest with a **real parser** for its format, and it is specified against
the dependency forms the CLI actually generates — one table, shared by `component new` and
`component upgrade`, so the two can never disagree again:

| Language | `--dep-source local` | `--dep-source registry` | `--dep-source pinned-rev` | Upgrade acts on |
|---|---|---|---|---|
| Java | `pom.xml`, `<version>` from the workspace | same, published GitHub Packages coordinate | **usage error** — Maven cannot express a monorepo-subdirectory git pin | `pom.xml` (XML-parsed, property-aware) |
| Python | `-e ../libs/python` | `edgecommons @ git+https://…@python-lib/vX.Y.Z#subdirectory=libs/python` | `edgecommons @ git+https://…@<rev>#subdirectory=libs/python` | `requirements.txt` (form-preserving; `--to` and `--to-rev`) |
| Rust | `path = "../libs/rust"` | `git = "…", tag = "rust-lib/vX.Y.Z"` | `git = "…", rev = "<rev>"` + a `dep:pinned-rev`-gated gitignored `.cargo/config.toml` sibling override | `Cargo.toml` (TOML-parsed; git-tag **and** git-rev forms; `--to` converts rev→tag, `--to-rev` moves the rev) |
| TypeScript | `file:../libs/ts` | `^X.Y.Z` under `@edgecommons/edgecommons` | **usage error** — npm git deps cannot address the `libs/ts` subdirectory | `package.json` (**scoped key**) |

**`pinned-rev`** (D-CLI-18) pins the **exact commit the CLI was built from** (`EDGECOMMONS_REV`, resolved
at CLI build time like `EDGECOMMONS_VERSION`), overridable with `--library-rev`. Because the embedded
templates and the rev come from the same commit, the pin is guaranteed to contain every library facade
the template calls — the correctness property the `registry` release tag cannot promise, since the tag
lags `main` (the failure that bit the dogfooding run). `local` stays the default (the monorepo-developer
common case); `pinned-rev` is the documented choice for a real, shippable component repo (SD-5).

`--dep-source registry` must produce a component that **resolves and builds** for all four languages —
the acceptance gate in §10 enforces it. Path dependencies are reported as "nothing to bump", never
silently rewritten. `--dry-run` prints the diff.

### 7.2 `component version` — a different verb from `upgrade`

`upgrade` moves a component to a new **edgecommons library** version. `version` sets the **component's
own** version, across whichever manifests declare it (`pom.xml`, `Cargo.toml`, `package.json`,
`gdk-config.json`, the recipe). Conflating the two is a trap the current CLI avoids only by not having
the second one.

The stated version is authoritative: `component version --to 0.3.0`. The CLI validates that it is
well-formed, monotonic, and not already published (against the release index, §9.1). It does **not**
derive a version from commit messages — no auto-semver in v1.

### 7.3 `component release` — the CLI produces; the runner publishes

This is a principle, not a preference, and it is the same line drawn in §8.6 for cloud apply:

> **Deterministic, credential-free work belongs in the CLI. Anything that needs a credential or mutates
> the world belongs in a runner.**

A release cut from a developer's laptop with publishing credentials has no provenance, no attestation,
and no reproducibility — precisely what the supply-chain evidence gate exists to prevent, and a direct
contradiction of the rule that the Runner port holds credentials and the tool never does.

So `component release` **builds the artifacts, computes their digests, and emits a release descriptor** —
and stops:

```jsonc
{
  "component": "telemetry-processor",
  "version": "0.3.0",
  "sourceCommit": "…",
  "artifacts": {
    "GREENGRASS": { "componentVersion": "0.3.0", "archive": "…", "sha256": "…", "recipe": "recipe.yaml" },
    "KUBERNETES": { "image": "ghcr.io/edgecommons/telemetry-processor", "digest": "sha256:…" },
    "HOST":       { "archive": "…", "sha256": "…" }
  },
  "supplyChain": { "sbom": null, "signature": null, "provenance": null }   // fields designed now, populated as they land
}
```

**Tagging, uploading, and opening the registry PR are the release *workflow's* job**, running the same
binary in CI — which is exactly the deck's rule that "CI is just the same binary invoked in a job". A
laptop dry-run therefore produces the exact bytes CI would, which is what makes the descriptor
reviewable before it is real.

Note the artifact coordinates are **per platform**. A single top-level digest would be meaningless: a
Greengrass artifact archive, an OCI image, and a HOST binary are three different objects.

This verb has a prerequisite the CLI cannot supply: **no EdgeCommons component publishes anything
today** — zero releases, zero tags, zero packages across all eight repos, with CI that builds and tests
but never packages. That is **RM-013**, and it is a separate initiative.

---

## 8. The deployment surface, at contract level

RM-012: *"The CLI is the product; the server is a shell around it."* `deployment validate | render |
plan | diff` must do the entire model→artifact job **with no server and no network**.

### 8.1 Verbs

| Verb | In | Out |
|---|---|---|
| `validate <definition>` | a deployment definition (folder or file) | Two stages: the definition's own schema, then **every rendered effective config** against the strict config schema (§6.1) + semantic rules (§6.2). |
| `lock` | definition + the release index | Resolves each pinned component version to an immutable digest and writes a lock file. **The only verb that touches the network** (§8.7). |
| `render --env <e> --target <t>` | definition, env, target | Native artifacts for the target **plus** the normalized plan. Writes to a render path; nothing is committed. |
| `plan` | definition, env, target | The normalized plan JSON alone — the common currency for validation, policy, CI, and the UI. |
| `diff --against <release-ref>` | a Git ref | The delta grouped **by consequence**: restart, storage, network, identity, permission, config, artifact, apply-order. |
| `release --stream artifact\|config` | definition + lock + the stream being promoted | Promotes **one stream**; writes a release manifest and a `ReleaseLock` that *correlates* the artifact and config streams without fusing them (§8.5). |

### 8.2 The hexagonal core and the five ports

The kernel is a library crate with no I/O. Five ports, each a trait with a zero-cost local adapter, and
**no cloud SDK linked above the port boundary** (P3):

```rust
pub trait GitPort      { /* definitions, layers, releases, evidence, approvals */ }
pub trait IdentityPort { /* who authored, who approved, who may edit which layer */ }
pub trait BlobPort     { /* artifacts, evidence bundles, render snapshots */ }
pub trait RunnerPort   { /* executes an apply; holds target credentials — the Studio never does */ }
pub trait TargetsPort  { /* the three control planes the renderers speak to */ }
```

Local adapters, which is what makes local development free: a local clone (Git), static dev users
(Identity), the filesystem (Blob), a subprocess (Runner), and `kind` / `greengrass-cli` local deployment
/ supervisord-in-containers (Targets).

### 8.3 Determinism rules

The deck asserts deterministic render; REVIEW.md flags that it is never operationalized, and names it a
load-bearing risk. It is a build gate here:

1. **Byte-stable serialization**: sorted keys, fixed indentation, LF endings, no locale-dependent
   formatting, and **no timestamps or hostnames in rendered output** (they belong in the release
   manifest, not the artifact).
2. **Effective config comes from `layered.rs`** (P4) — not a second merge implementation.
3. **Golden-file suites per renderer**, with `bottling-company-test/sites/dallas-site` as the first
   oracle: regenerate its supervisord confs, per-line config JSONs, and ConfigComponent catalog from a
   definition and compare **byte for byte**.
4. **The renderer version is an input to every hash.** A renderer bump invalidates hashes by
   construction, and that is a stated, tested behavior rather than a surprise.

### 8.4 `studio serve`

The same kernel behind an `axum` server with the SPA embedded. In v1 this is a **compiling seam**: the
subcommand exists, the ports are wired, the server is not built. Nothing in the kernel may assume a
server exists.

### 8.5 `release` — two streams, not one (REVIEW #2, decided 2026-07-11)

**Decision: do not fuse.** A release is **two independently versioned, independently reconciled streams**:

| Stream | What it promotes | Reconciled by |
|---|---|---|
| **Artifact** | A component's binary/image at a pinned version + digest (§8.7). | A platform deployment — on Greengrass, a per-thing deployment carrying a new `componentVersion`. |
| **Config** | The effective config. | A delivery adapter chosen by the component's **config source** — catalog push, config-only deployment, ConfigMap, staged file, env, or shadow (§8.5.3). |

The `ReleaseLock` is a **correlation and evidence envelope over both — not an atomic apply unit.** It
records what was in effect together; it does not force the two to move together. A config change ships
without reshipping the artifact, and the reverse. Each stream carries its **own drift signal and its own
rollback target**, which the deck's four-way drift taxonomy already accommodates.

`deployment release --stream artifact|config` promotes one stream; the lock correlates them.

### 8.5.1 Greengrass: per-thing deployments only (REVIEW #3, decided 2026-07-11)

**Thing groups are not used.** IIoT edge devices each carry a **unique configuration**, and the members of
a thing group necessarily share **one deployment document** — so a group cannot express per-device config.
Grouping is the wrong primitive for this fleet, whatever else a deployment carries.

Consequences, all of them simplifying:

- A definition's `nodes[]` map **1:1 onto Greengrass deployments**; `targetArn` is a thing ARN. The
  thing-group **union-semantics problem disappears entirely**, and with it the modeling constraint that
  REVIEW.md named as the second load-bearing risk.
- The plan carries **one apply record per node**, and partial failure is per-node — which is precisely
  what the deck's staged, selectable rollout wants. Per-thing is a feature here, not a tax.
- N devices means N deployments. That is an operational cost (N API calls, N revisions), not a
  correctness one, and it is the honest price of per-device configuration.

### 8.5.2 Separation is a model invariant, not a transport one

Greengrass does **not** technically fuse config and binary. A deployment can carry a new
`configurationUpdate` against an unchanged `componentVersion` (config-only), or a new `componentVersion`
retaining the existing config (binary-only). **It is the tooling and the UI that combine them**, not the
mechanism.

The rule that follows:

> **The model, the release streams, the evidence, the drift signals, and the rollback targets keep config
> and artifact strictly distinct. The platform adapter MAY coalesce them into a single native deployment
> when that is the right thing for a given command.**

So a command that promotes both streams at once is free to become **one** Greengrass deployment carrying
both a new `componentVersion` and a new `configurationUpdate` — while the release objects above it remain
two independently versioned streams with two rollback targets. This is where the `ReleaseLock`'s
correlation envelope earns its keep: a coalesced apply record references *both* stream releases, so the
audit trail still says which config and which artifact were in effect, and either can still be rolled back
alone (the adapter emits a deployment that reverts one and retains the other — the capability Greengrass
already has).

### 8.5.3 Config delivery is per provider — ConfigComponent is preferred, not required

**`config-component` is not a dependency of the deployment system.** It is the preferred config source, and
the design should say so — but a customer remains free to use the native Greengrass config source, or any
other supported provider. The Studio must not quietly make one component mandatory.

So the config stream has **one model and N delivery adapters**. The effective config is computed **once**,
by `layered.rs` (P4); only *how it reaches the device* varies. The component's declared config source —
already part of the runtime contract (`-c/--config <SOURCE>`) and therefore part of the definition — selects
the adapter:

| Config source | How the config stream is delivered | Picked up live? |
|---|---|---|
| `CONFIG_COMPONENT` *(preferred)* | A catalog lineage push. **No platform deployment at all**, on any platform. | Yes — a catalog push is the hot path |
| `GG_CONFIG` | A config-only Greengrass deployment: unchanged `componentVersion`, new `configurationUpdate`. Still per-thing. | **Not reliably** — see below |
| `CONFIGMAP` | The Kubernetes renderer's ConfigMap (whole-volume mount, no `subPath`). | Yes — the `..data` swap is watched |
| `FILE` | A config file staged into the HOST bundle, checksummed. | Yes — the file is watched |
| `ENV` | Environment in the unit/manifest. | **No** — an env change requires a restart |
| `SHADOW` | A shadow document update. | Yes — via the shadow delta |

This is a strictly better shape than a ConfigComponent-first design: it keeps the deployment system free of
any hard dependency on a specific component, and it agrees with REVIEW #6, which already preferred rendering
Git content into the existing file/ConfigMap sources over building a `GitCatalogSource` first.

A new semantic rule falls out: **a config source must be legal for the platform it is deployed on**
(`EC2009`, §6.2) — `CONFIGMAP` only on KUBERNETES, `GG_CONFIG` only on GREENGRASS — and `CONFIG_COMPONENT`
already carries the bootstrap-loop guard (`EC2007`).

### 8.5.4 The restart caveat, which is why the matrix has a third column

A pure config update does **not reliably restart the component**. That is precisely the problem EdgeCommons'
**dynamic config (hot reload)** exists to solve — a component sourcing config through the library can pick a
change up live. But as the matrix shows, **that capability is a property of the config source, not of the
platform**: a `CONFIGMAP` or `FILE` change is watched, an `ENV` change is not, and a Greengrass
`configurationUpdate` is not reliably a restart either.

Concretely: **restart impact is a first-class field of the plan**, computed **per component, per config
change, from that component's config source**. The deck's `diff` already groups changes by consequence and
already has a **restart** group — this is what populates it. `deployment plan` therefore states, for each
config change, whether it is picked up live or forces a restart, and an operator sees the blast radius
*before* applying rather than discovering it in production.

### 8.5.5 The compatibility guard — derive, don't declare (OQ-6, decided 2026-07-11)

Two-stream buys independence, and independence has one sharp edge: **nothing stops a config release from
shipping config that the deployed binary cannot parse.** With six delivery paths (§8.5.3) this is not a
Greengrass concern — a config release can reach a stale binary on any platform.

The obvious guard is a declared floor: the config release states `requiresArtifact: ">= 0.3.0"` and
`validate` enforces it against the artifact stream's pin. That works, but it is an **assertion a human must
remember to maintain**. An author who adds a config key and forgets to raise the floor ships the bug the
guard existed to prevent, and the granularity is coarse — a whole component, not the offending key.

**The decision is to derive compatibility instead of declaring it.** `deployment validate` checks the
effective config against **the config schema published by the exact component version being deployed**:

1. The **release descriptor** (RM-013, §7.3) carries a **`configSchema`** — the schema *that version*
   accepts.
2. **`deployment lock`** fetches it alongside the digest and commits it with the lock, so `validate` stays
   offline (§8.7, P2). The schema is an input to the render hash like any other.
3. **`deployment validate`** validates the effective config against it. A key that only exists in 0.4.0
   fails precisely against a pinned 0.3.1 — *"`pipeline.window` is not accepted by telemetry-processor
   0.3.1"* — rather than a coarse "your floor says 0.3.0, so this is fine."

`requiresArtifact` **survives as a fallback**, not the primary mechanism, for the two cases a schema cannot
carry: a key whose *meaning* changed while its shape did not, and a component that does not yet publish a
schema. Where neither a schema nor a floor is available, `validate` **warns** and names the reason —
consistent with RM-013's degradation rule.

**This closes a hole that exists today, independent of deployments.** The canonical schema is strict at the
top level, but `component.global` is `additionalProperties: true` with **zero declared properties** — so a
component's own config is validated against *nothing*, and no component repo ships a schema. Today a typo in
a `telemetry-processor` pipeline is caught by no tool at any stage. The same per-version schema therefore
pays twice: it powers this guard, **and** it gives `component validate` (§6) real component-config
validation, which is the capability an author actually wants.

**The cost, stated plainly:** every component repo must author and publish a config schema. That is eight
repos of new work, and it belongs to **RM-013**, not to the CLI. Until a component publishes one, its config
is unvalidated and `validate` says so out loud rather than implying coverage it does not have.

### 8.6 Deviation to acknowledge: `deploy --target` is removed in v1

Today `edgecommons deploy --target <arn>` shells out to `aws greengrassv2 create-deployment`. That is an
**apply**, and apply belongs behind the Runner/Targets ports — the deck is explicit that apply runs in a
runner holding the target credentials, never in the Studio process. v1 keeps the build/publish half as
`component package [--publish]` (still `gdk`, still a shell-out) and **drops the cloud-deployment half**
until the Targets port lands.

The practical cost is near zero and should be stated plainly: that command **cannot run on a freshly
scaffolded component today anyway** — every template ships `"version": "NEXT_PATCH"` and `deploy`
hard-rejects `NEXT_PATCH` (DEF-6). Its one genuinely valuable behavior — refusing to deploy an unlocked
version — is not lost: it is the ancestor of the release-lock gate and moves into `deployment` validation.

### 8.7 Pins and the lock file — how "no network" becomes literally true

A definition **pins a component version**; a **lock file records the resolved digest**. This is the
`Cargo.lock` pattern, and it is what makes RM-012's "with no server and no network" a fact rather than an
aspiration:

- `deployment lock` is the **one** verb that reaches the network. It resolves each pinned version against
  the release index (§9.1) and writes the digests into a lock file **committed to Git** — **together with
  each pinned version's published config schema** (§8.5.5), so the compatibility check is offline too.
- `validate`, `render`, `plan`, and `diff` are then **pure functions over files already in Git**. An
  air-gapped site needs a definition, a lock, and a Git bundle — nothing else.
- Determinism (§8.3) follows for free: every hash input is committed, so a render is reproducible from
  the definition commit plus the renderer version.

**Degradation is explicit.** Until components actually publish (RM-013), a definition may hand-pin a
version with no resolvable digest. `deployment validate` then emits a **warning**, not an error, naming
the reason ("no release index published for `<component>`"). When the index appears, the identical code
path begins enforcing. No redesign, no flag.

---

## 9. `registry` and `doctor`

**`registry list`** preserves today's behavior (the `gh`-authenticated read of the private
`edgecommons/registry` catalog, `$EDGECOMMONS_REGISTRY_URL`, a local path, `--json`) and fixes the filter
help, which advertises three of the six categories the schema actually defines. It validates the catalog
against `registry.schema.json` rather than checking that a `components` key exists.

### 9.1 The registry is three layers, not one

The Studio's deployment model pins `artifact: { version, digest }`, and `registry/components.json` has
**no version, artifact, or digest field**. The answer is *not* to add those fields to the catalog entry:
that is mutable per-release data in a hand-edited file (stale by the second release), it can only express
the *current* version while definitions pin arbitrary historical ones, and a single top-level digest is
meaningless when a component ships a different artifact per platform.

| Layer | What it is | Who writes it |
|---|---|---|
| **Discovery** — `components.json` | What exists: repo, language, category, platforms. Slow-moving. | Humans (unchanged) |
| **Release index** — `releases/<component>.json` | Every published release: version, source commit, per-platform artifact coordinates + digests, supply-chain refs. | **CI only** — pushed by each component's release workflow; never hand-edited; schema-validated by registry CI |
| **Pin + lock** | The definition pins a version; the lock file records the resolved digest (§8.7). | The definition author; `deployment lock` |

`registry versions <component>` reads the index. This resolves the design's OQ-3 and is scoped as
**RM-013**, because its true prerequisite is not a schema change — it is that **no EdgeCommons component
publishes anything at all today** (zero releases, zero tags, zero packages across all eight repos).

**`doctor`** becomes platform-aware and honest. It takes `--platform` (defaulting to all), checks only
what the selected platforms need, **verifies versions** (Rust ≥ MSRV 1.85, Java 25, Node ≥ 18, `gdk`),
adds the tools the current list omits — `gh` (which `registry list` requires), `docker`, `kubectl`,
`helm` — and **exits non-zero when something required is missing**. Today it always exits `0`, which
makes it useless in CI.

---

## 10. Testing and acceptance gates

The acceptance gate is the org's standing one, not "the Rust tests pass":

| Gate | What it is |
|---|---|
| **Scaffold → build → run** | Every template in the matrix (§5.1), for **both** dep-sources, scaffolded, compiled, and run across the testable deployment options. This is the gate that would have caught DEF-1, DEF-2, and DEF-3. |
| **Golden files** | Template output and (later) renderer output, byte-compared. |
| **Determinism** | Render twice, compare bytes; render with a bumped renderer version, assert the hash changes. |
| **Defect regressions** | One test per row of §12. |
| **Coverage** | ≥ 90% line, matching the four libraries. The Python CLI has **no coverage gate at all** today, and its CI runs `pytest` on Python 3.12 only. |
| **Schema drift** | The embedded config schema matches `schema/edgecommons-config-schema.json`. |

Note the historical evidence for why the matrix gate matters: TypeScript generation is never tested
today, and `_bump_package_json` has no test at all — which is exactly why both are broken.

---

## 11. Architecture and delivery

### 11.1 Crate layout

A Cargo workspace under `cli/`, replacing the Python tree in place (`core/cli/`), where the templates,
the canonical schema, and `libs/rust` are all path-reachable.

| Crate | Responsibility | May depend on |
|---|---|---|
| `ec-cli` (bin) | `clap` derive, output rendering, exit codes. Thin. | all below |
| `ec-diag` | The diagnostic model + human/JSON renderers. | — |
| `ec-scaffold` | Embedded templates, manifest v2, generation pipeline, wizard. | `ec-diag` |
| `ec-validate` | Embedded schema, semantic rules, artifact lint. | `ec-diag`, `edgecommons` (`uns.rs`, `config/layered.rs`) |
| `ec-deploy` | The hexagonal kernel: model, renderers, plan, the five port traits. **No I/O.** | `ec-validate`, `edgecommons` |
| `ec-adapters` | Local adapters + `gdk`/`greengrass-cli`/`kubectl`/`docker` shell-outs. **The only crate that may link a cloud SDK, and only in the Greengrass adapter.** | `ec-deploy` |
| `ec-studio` | The `axum` server shell. Seam only in v1. | `ec-deploy` |

Rust edition 2024, MSRV 1.85 — matching `libs/rust`.

### 11.2 Distribution

The CLI is finally **published**: today `release.yml`'s `cli/v*` tag prefix only builds a dist and
uploads it as a workflow artifact, so there is no install path but a checkout. Ship static binaries per
OS/arch from the existing tag prefix, plus `cargo install`. This is the payoff RM-012 names — scaffolding
a Java or TypeScript component stops requiring a Python runtime.

### 11.3 Phases

| Phase | Delivers | Done when |
|---|---|---|
| **P0** | Workspace, `clap` skeleton, `ec-diag`, `doctor`, `completions`. | `doctor` is platform-aware and exits non-zero. |
| **P1** | `ec-scaffold`: manifest v2, embedded templates, packs, wizard, `component new`, `template list\|show`. | The scaffold→build→run matrix is green for the four existing `service` templates on all platforms. |
| **P2** | `ec-validate`: schema + semantic + artifact lint; `component validate`. | Every defect in §12 that is a validation defect has a regression test. |
| **P3** | `registry list\|show\|versions`; `component upgrade` (all four dep forms), `component version`, `component package`, `component release` (descriptor only, §7.3). | Both dep-sources build in all four languages. **The Python `cli/` is deleted, and every doc that describes it is updated in the same change.** `component release` is usable by RM-013's release workflow. |
| **P4** | `ec-deploy`: the model, `validate\|lock\|render\|plan\|diff`, the five port traits, local adapters, the **HOST renderer first** (per REVIEW.md's slice-1 amendment). | `bottling-company-test/sites/dallas-site` is regenerated byte-for-byte from a definition. |
| **P5** | The Greengrass (per-thing) and Kubernetes renderers; the `studio serve` seam. | — |
| **P6** | `deployment release` — both streams (§8.5). **No longer blocked**; REVIEW #2/#3 landed 2026-07-11. Gated instead on RM-013, since an artifact-stream release needs artifacts that exist. | An artifact release and a config release promote independently, each with its own rollback target. |

New templates (`rust/protocol-adapter`, `*/processor`, `*/sink`) are template work that can land any time
after P1 with no CLI change — that is the point of §5.

### 11.4 Sequencing note

The Studio deck places the Python→Rust port in **slice 3**, after the deployment model and the HOST
renderer. This plan pulls it forward: P0–P3 deliver a correct, publishable component CLI first, and P4
then builds the deployment kernel on top of a validation engine that already exists. The alternative —
building `deployment validate` against a CLI that cannot validate a config — means writing the validation
engine anyway, just in a worse order.

---

## 12. Defect register — requirements harvested from bugs

Every row is a live defect in the shipped Python CLI, verified against source. Each becomes a regression
test. This register — not the current pytest suite — is the behavioral oracle.

| # | Defect | Evidence | Fixed by |
|---|---|---|---|
| DEF-1 | `--dep-source registry` + Rust emits a Cargo dep on git tag `rust-lib/v0.1.0`, **which does not exist** (repo has `v0.1.1`, `v0.2.0`; libs are at 0.2.0). | `create_component.py:78` | §5.7 (version resolved from the workspace), §10 |
| DEF-2 | The generated **Python Greengrass component fails its install lifecycle on device** — the recipe installs `greengrass_commons-0.0.10038883-py3-none-any.whl`, a pre-rebrand wheel that is never produced. | `templates/python/recipe.yaml:79`, `templates/python-protocol-adapter/recipe.yaml:59` | Template fix + §10 |
| DEF-3 | `upgrade` is a **silent no-op for every TypeScript component** (scoped key vs bare key). | `upgrade.py:123` vs `templates/typescript/package.json:14` | §7 |
| DEF-4 | `upgrade` **corrupts Python components** — rewrites the `git+https` form to `edgecommons==X`. | `upgrade.py:85-86` | §7 |
| DEF-5 | `upgrade` cannot bump the git-tag Cargo dep that `create-component` itself emits. | `upgrade.py:100-115` | §7 |
| DEF-6 | `deploy --target` **cannot run on a freshly scaffolded component**: every template ships `NEXT_PATCH`, which `deploy` hard-rejects. | `templates/*/gdk-config.json:5`, `deploy.py:96-100` | §8.6 |
| DEF-7 | `jsonschema` is a declared runtime dependency **used by no command** — in a CLI whose biggest gap is that it cannot validate a config against the canonical schema. | `pyproject.toml:9` | §6 |
| DEF-8 | Two complete templates (`java-protocol-adapter`, `python-protocol-adapter`) are **unreachable**: absent from the language dict and from the wheel bundle. | `create_component.py:46-51`, `setup.py:14` | §5.1, §5.2 |
| DEF-9 | `lint_least_privilege` (the `RequiresPrivilege: true` check) exists but is **never wired into `validate`**. | `recipe_lint.py:25-40` | §6.3 |
| DEF-10 | `doctor` never exits non-zero, checks no versions, and omits `gh` (which `list-components` requires), `docker`, `kubectl`, `helm`. | `doctor.py:7-16` | §9 |
| DEF-11 | `edgecommons add <name>` is documented twice as a command; it does not exist. | `docs/ECOSYSTEM.md:80,149` | Delete the promise |
| DEF-12 | Greengrass artifacts are emitted for HOST-only scaffolds; HOST gets no artifacts at all. | `templates/*/edgecommons-template.json` | §5.5 |
| DEF-13 | The crate/bin name is mangled with no override: `-n …EthernetIpAdapter` → `ethernetipadapter` (dots stripped, no separator), while the ecosystem uses kebab (`ethernet-ip-adapter`). | `ethernet-ip-adapter/CLI-DOGFOODING.md` #1 | §5.7 (kebab `BINNAME` + `--bin-name`/`--crate-name`), D-CLI-17 |
| DEF-14 | The output directory is the PascalCase short name (`./EthernetIpAdapter`), not the kebab repo name; no `--dir`. | `CLI-DOGFOODING.md` #2 | §5.7 (default dir = `BINNAME`; `--dir`), D-CLI-17 |
| DEF-15 | Neither `--dep-source` matches the sibling convention (rev-pin in `Cargo.toml` + gitignored `.cargo` override); `registry`'s release tag lags the facades the template calls. | `CLI-DOGFOODING.md` #3 | §7.1 (`pinned-rev`), D-CLI-18 |
| DEF-16 | A bucketless `gdk-config.json` is left silently empty; the only signal is a warning, and the miss surfaces at `gdk component publish` weeks later. | `CLI-DOGFOODING.md` #4 | §5.7 (sentinel + `EC3007` validate error), D-CLI-20 |

Findings #5–#12 of the dogfooding log are **template-parity** requirements (metric families, the `sb/*`
command surface, Diátaxis docs, CI, integration tests, governance files, edge-console panels, lockfile
policy) rather than CLI defects; they are specified in `DESIGN-cli-scaffold-parity.md` (R5–R12) and
realized by D-CLI-19/-21/-22/-23.

---

## 13. Decision register

| # | Decision | Rationale |
|---|---|---|
| D-CLI-1 | **Greenfield, not a port.** | The port would carry a Greengrass-era product, a reflective plugin framework, and 12 defects. RM-012 already commits to the Rust binary. |
| D-CLI-2 | **The defect register + the scaffold→build→run gate is the oracle — not the pytest suite.** | The suite is green while the code is broken; the broken paths are the untested ones. Corrects RM-012's stated approach. |
| D-CLI-3 | **Noun–verb surface, clean break, no aliases.** | The CLI is unpublished with no user base (RM-012). The window is now. `validate` vs `deployment validate` is otherwise a permanent trap. |
| D-CLI-4 | **Templates are language × kind**, discovered from manifests; kinds are `service`, `protocol-adapter`, `processor`, `sink`. | Two archetype templates already exist and are orphaned; the vocabulary matches the registry's categories. Adding a template needs no CLI change. |
| D-CLI-5 | **Lives in `core/cli/`, replacing the Python tree.** | Templates, the canonical schema, and `libs/rust` (which the CLI must *call*, per P4) are all path-reachable; the `cli/v*` release prefix already exists. |
| D-CLI-6 | **The validation engine is built once and shared** by `component validate` and `deployment validate`. | The Studio requires effective-config validation; the component author wants the same thing. Two implementations would drift. |
| D-CLI-7 | **`release` promotes two independent streams** (artifact, config); the `ReleaseLock` correlates them and does not fuse them. **Greengrass deploys per-thing only.** | REVIEW #2 and #3, decided 2026-07-11. Fusing the two in the *model* is the Greengrass-tooling coupling RM-002 rejects; thing groups cannot express the per-device config that IIoT edges require. Unblocks the verb — it is no longer deferred (§8.5). See D-CLI-13 for what the *adapter* may still combine. |
| D-CLI-8 | **`deploy --target` (cloud apply) is dropped in v1**; `component package [--publish]` keeps the build/publish half. | Apply belongs behind the Runner/Targets ports. The dropped command cannot run on a fresh scaffold today (DEF-6). Its version-lock gate is preserved in `deployment` validation. |
| D-CLI-9 | **The port order is pulled forward** relative to the deck's slice 3. | `deployment validate` needs a validation engine that does not exist; building it first is strictly cheaper. |
| D-CLI-10 | **The CLI produces; the runner publishes.** `component release` builds, digests, and emits a descriptor — it never tags, uploads, or pushes. | A release cut from a laptop with credentials has no provenance and no attestation. Generalizes D-CLI-8: deterministic and credential-free belongs in the tool; credentialed and world-mutating belongs behind a port. CI runs the same binary. |
| D-CLI-11 | **The registry splits into discovery / release index / pin+lock** (§9.1); `version` and `digest` are *not* added to the catalog entry. | Per-release data in a hand-edited catalog is stale by the second release, cannot express historical pins, and a single digest is meaningless across three platforms. |
| D-CLI-12 | **`deployment lock` is the only networked verb.** | Makes RM-012's "no server and no network" literal for `validate\|render\|plan\|diff`, and puts every hash input in Git, which is what §8.3's determinism actually requires. |
| D-CLI-13 | **Stream separation is a *model* invariant, not a *transport* one.** The model, releases, evidence, drift, and rollback keep config and artifact distinct; a platform adapter **may coalesce** them into one native deployment when that suits the command (§8.5.2). | Greengrass does not actually fuse the two — its *tooling* does. Forbidding a combined deployment would impose a restriction the platform never had, while fusing the *model* would import the coupling RM-002 rejects. Separate where it buys reasoning; combine where it buys an apply. |
| D-CLI-14 | **Restart impact is a first-class field of the plan** — computed per component, per config change, **from that component's config source**. | A pure config update does not reliably restart a component; dynamic config/hot reload is what addresses that, and **whether it applies is a property of the config source, not the platform** (`FILE`/`CONFIGMAP` are watched, `ENV` is not, a GG `configurationUpdate` is not reliably one). The deck's `diff` already has a **restart** consequence group; this populates it. |
| D-CLI-16 | **Config/artifact compatibility is *derived*, not declared.** Components publish a **per-version config schema** in the release descriptor; `lock` commits it; `validate` checks the effective config against the schema of the **exact version being deployed**. `requiresArtifact >= X` survives only as a fallback (§8.5.5). | A declared floor is an assertion a human must remember to bump, and it is coarse. A schema check is automatic and names the offending key. It also closes a hole that exists today: `component.global` is `additionalProperties: true` with no declared properties, so component config is validated by nothing at any stage. |
| D-CLI-15 | **`config-component` is preferred, not required.** The config stream is **provider-agnostic**: one model, one `layered.rs` computation, and a **delivery adapter per config source** (§8.5.3) — catalog push, config-only deployment, ConfigMap, staged file, env, or shadow. | Customers may use the native Greengrass config source or any supported provider; the deployment system must not make one component mandatory. Also agrees with REVIEW #6, which already preferred rendering Git content into the existing file/ConfigMap sources over building `GitCatalogSource` first. |
| D-CLI-17 | **Names are kebab by default; the Greengrass component name stays PascalCase.** The crate/`[[bin]]`, Maven artifact, npm name, Python module dir, output directory, and every deploy-artifact name derive from a single acronym-aware kebab of the short name (`--bin-name`/`--dir` override); `COMPONENTFULLNAME` stays PascalCase reverse-DNS. | Every sibling repo and UNS token is kebab (`modbus-adapter`); the old mangled default (`ethernetipadapter`) violated the convention and every dogfooded scaffold hand-fixed it (DEF-13/-14). The CLI is pre-1.0 with one user — change the default now. |
| D-CLI-18 | **`--dep-source pinned-rev` pins the CLI's own build commit** and emits a gitignored `.cargo/config.toml` sibling override (Rust) / documents the editable install (Python); Java/TS reject it. `local` stays the default. | The embedded templates and the pinned rev come from the same commit, so the pin contains every facade the template calls — the correctness the `registry` tag cannot give (its tag lags `main`, DEF-15). Maven/npm cannot express the monorepo-subdirectory git pin, so it is an honest usage error there, not a silent fallback. |
| D-CLI-19 | **The archetype-parity floor.** A template is: runnable against its in-process sim, canonical on the published contracts (SOUTHBOUND §5 metrics, §2.2 command shapes), complete on repo hygiene (docs/CI/governance/tests/panels), and deliberately generic on protocol specifics (no protocol families beyond two worked ones, no real backend, one-page sim browse). | Full literal parity with a reference adapter is impossible in a generic template (protocol-named families, protocol browse). This line is where "minimal archetype" ends and the reference adapter begins — stated so it is enforced in review, not re-litigated per template (SD-1 Option A). |
| D-CLI-20 | **A bucketless Greengrass scaffold writes a visible sentinel** (`edgecommons-set-artifact-bucket`) and `component validate` **errors** on it (`EC3007`). | An empty field reads as intentional; a sentinel + a validate error catches the miss at authoring/CI rather than at `gdk component publish` (DEF-16). |
| D-CLI-21 | **The license is the author's choice.** `--license <SPDX\|none>` (default `none`) writes the LICENSE file and the manifest license field; no default license is stamped. | A scaffold is the *author's* component; baking the EdgeCommons license (BUSL-1.1) into third-party code is wrong. The old templates silently stamped `Apache-2.0` — a small defect this removes. Internal scaffolds pass `--license BUSL-1.1`. |
| D-CLI-22 | **Lockfiles are instruct-and-validate, never embedded.** No template ships a `Cargo.lock`/`package-lock.json`; `component new` prints a "commit the lockfile" next step and `component validate` **warns** (`EC4008`) when a Rust/TS component has none. | A template cannot ship a *valid* lockfile (the graph depends on dep-source and the resolution moment), and generating one needs a toolchain + network — violating the offline principle (P2). |
| D-CLI-23 | **`sb/pause`/`sb/resume` are promoted into the SOUTHBOUND contract** (§2.2) and shipped by the `protocol-adapter` scaffold, alongside the `reconnect`/`repoll` lifecycle-control family. | They were a single-adapter extension (D-EIP-3); rather than back-door them into the template only, SD-2 promotes them into the shared contract so the scaffold and the doc agree. |

---

## 14. Open questions

- **OQ-1 — RESOLVED 2026-07-11 (REVIEW #2).** Two-stream; do not fuse. See §8.5.
- **OQ-2 — RESOLVED 2026-07-11 (REVIEW #3).** Greengrass deploys **per-thing only**; thing groups are not
  used. See §8.5.1. The definition schema is now unblocked for P4.
- **OQ-6 — RESOLVED 2026-07-11.** Yes to a compatibility guard, but **derived, not declared**: validate the
  effective config against the config schema published by the exact component version being deployed
  (§8.5.5, D-CLI-16). `requiresArtifact >= X` is kept only as a fallback. The prerequisite — components must
  publish a per-version config schema — is scoped to **RM-013**.
- **OQ-7 — RESOLVED 2026-07-11: yes.** The four libraries validate the component's own config against its
  schema **at startup (fail fast) and on hot reload (reject, keep last good)**. Scoped as **RM-014** — it is
  a four-language parity change and needs a core-library design doc of its own. The consequence for this
  document is in §5.6: **templates ship a `config.schema.json` and its wiring by default**, so a scaffolded
  component is validated from day one.
- **OQ-3 — RESOLVED.** Component pins had no catalog to resolve against. Answer: the three-layer registry
  (§9.1) plus `deployment lock` (§8.7), with the underlying prerequisite — components do not release at
  all — scoped as **RM-013**. Until that lands, unverifiable pins warn rather than block.
- **OQ-4 — `component package` for HOST/Kubernetes.** Should it shell out to `docker build`, or stay
  Greengrass-only (`gdk`) and leave container builds to CI? (Interacts with RM-013: the org has never
  published an image.)
- **OQ-5 — Policy and Sign.** The deck's pipeline has both as stages (Rego compiled to WASM, evaluated
  in-process so it works offline), but neither has a CLI verb. `deployment policy` / `deployment sign`?
