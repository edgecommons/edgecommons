# Design — CLI scaffold parity: closing the ethernet-ip-adapter dogfooding gaps

> **Status: ACCEPTED (2026-07-19) — scope decisions resolved by the user; in implementation.**
>
> **Resolved scope decisions:**
> - **SD-1 → Option A.** Canonical `southbound_health` (exact §5 set) + a generic operational-metrics
>   module with two worked families and an explicit "add your protocol's families here" extension
>   point; the full generic `sb/*` command family. Not full literal 5-family parity.
> - **SD-2 → Include AND promote.** `sb/pause`/`sb/resume` ship in all 4 adapter templates **and** are
>   promoted into `core/docs/SOUTHBOUND.md` §2.2 as a real ecosystem-contract change (core doc +
>   templates together). They are therefore part of the standardized `sb/*` family everywhere below.
> - **SD-3 → Change the defaults.** Kebab crate/bin/dir/artifact naming is the default; the Greengrass
>   component name (`COMPONENTFULLNAME`) stays PascalCase reverse-DNS.
> - **SD-4 → Governance in all 16 + `--license` (default `none`).** AGENTS/CLAUDE/DESIGN skeletons in
>   all 16 templates; `--license <SPDX|none>` flag with embedded BUSL-1.1/Apache-2.0/MIT texts; the
>   current hardcoded `Apache-2.0` stamps are removed when `--license` is absent.
> - **SD-5 → recommendation.** `local` stays the default dep-source; `pinned-rev` is the documented
>   real-component choice, added to the wizard and the CI scaffold→build matrix.
> - **SD-6 → recommendation.** Lockfiles are never embedded; instruct-and-validate (next-steps
>   epilogue + `component validate` warning).
>
> _(original proposal preamble follows)_

> **Status: PROPOSED — awaiting user sign-off on the Scope Decisions below.**
> Extends `core/docs/platform/DESIGN-cli.md` (decision register D-CLI-1…D-CLI-16, defect register
> §12). Nothing here contradicts that register; new decisions are numbered D-CLI-17…D-CLI-23 and the
> dogfooding findings become DEF-13…DEF-16 + parity requirements R5–R12.
>
> **Sources (re-read, not recalled):** `ethernet-ip-adapter/CLI-DOGFOODING.md` (all 12 findings);
> `core/cli/crates/ec-scaffold/src/{generate,manifest,catalog,upgrade}.rs` + `build.rs`;
> `ec-cli/src/{cli.rs,commands/component.rs}`; `core/docs/platform/DESIGN-cli.md`;
> `core/docs/SOUTHBOUND.md` (§2.2, §5); the full `core/templates/rust-protocol-adapter/` set and the
> java/python/typescript adapter templates; `modbus-adapter/modbus_adapter/{metrics,command_service}.py`
> + `docs/` + `.github/workflows/ci.yml`; `ethernet-ip-adapter/crates/ethernet-ip-adapter/src/{commands,metrics}.rs`,
> `.cargo/config.toml`, root `Cargo.toml`, `.github/workflows/ci.yml`;
> `edgecommons/.github/.github/workflows/component-ci.yml`; `core/.github/workflows/cli.yml`.
>
> **Verified ground truth used throughout:** the template matrix is **16 templates (4 languages × 4
> kinds)**, all discoverable (the `core/CLAUDE.md` claim that TS is service-only is stale — a doc fix
> in this plan). `commands.register_panel` exists in **all four** core libraries (verified in
> `libs/{java,python,rust,ts}`), so the panel gap (#11) is implementable at full parity. The rust
> template's `.gitignore` already deliberately does **not** ignore `Cargo.lock`.

---

## SCOPE DECISIONS FOR THE USER (read first — these change effort materially)

### SD-1 — How far does adapter parity go into the base templates? (findings #5, #6)

The dogfooded adapter ended up with **5–6 protocol-named metric families with dozens of
`(total, interval)` measures** (`EtherNetIpConnection/Inventory/Poll/Publish/Command/Io`, mirroring
`ModbusConnection/…`) and **nine command verbs**. A *generic* template cannot ship protocol-named
families (`ModbusPoll` is meaningful; `AdapterPoll` for a sim is teaching material), so full literal
parity is impossible by construction; the question is where the line sits.

- **Option A (recommended): canonical floor + taught pattern.**
  - `southbound_health` brought to the **exact SOUTHBOUND.md §5 set** (`connectionState`,
    `publishLatencyMs`, `pollLatencyMs`, `readErrors`, `staleSignals`, + optional `reconnects`) —
    today's template *diverges* from the canonical doc (omits `publishLatencyMs`/`staleSignals`,
    adds ad-hoc measures). This is a straight defect fix.
  - Plus a **generic operational-metrics module** implementing the reference pattern — the
    `(total, interval)` counter pair, interval reset on emit, low-cardinality dimensions
    (`instance`, `result`, `verb`) — shipping **two worked families** (`{Adapter}Connection`,
    `{Adapter}Command`, named from the component) with an explicit "add your protocol's
    `Inventory`/`Poll`/`Publish` families here" teaching section, in all 4 languages.
  - Command surface: the **full generic `sb/*` family** (see SD-2 for pause/resume): `sb/status`,
    `sb/read`, `sb/write` (§2.2 batch shape, confirmed, allow-listed), `sb/signals`, `sb/browse`
    (seam method with a `BROWSE_UNSUPPORTED` default), `reconnect`, `repoll` — with the D-EIP-13
    instance-routing rule and the standardized error codes.
- **Option B: full family parity.** All five families with the full modbus/EIP measure lists in all
  4 languages, driven from the sim. ~3–4× the template code and test volume of Option A, most of it
  measures a real adapter renames anyway.
- **Consequence:** Option A closes the dogfooding complaint ("one metric, one command") and makes the
  next adapter's day-1 delta small and *named*; Option B makes the templates near-copies of the
  reference adapters at significant maintenance cost. **Recommendation: A.**

### SD-2 — `sb/pause` / `sb/resume` in the templates? (finding #6)

These verbs are a **deliberate ethernet-ip-adapter extension** (D-EIP-3); `SOUTHBOUND.md` §2.2 does
**not** define them and neither reference adapter has them. Putting them in the templates would
de-facto promote the extension into the ecosystem contract via the back door.
**Recommendation: exclude them from the templates for now**, and file the SOUTHBOUND.md §2.2
promotion (the D-EIP-3 "candidate for core promotion") as a separate, explicit contract change; if
you'd rather promote now, the promotion PR (core doc + templates) is a small add-on phase.
**I will not silently include or exclude them — pick one.**

### SD-3 — Default naming/output-dir behavior change (findings #1, #2)

Fixing the kebab derivation changes **default** behavior: `com.example.MyComponent` today produces
crate/bin `mycomponent` in `./MyComponent/`; after the change it produces `my-component` in
`./my-component/`. Overrides (`--bin-name`, `--dir`) exist either way.
- **Recommendation: change the defaults.** The CLI is pre-1.0 with essentially one user; the current
  default is the documented ecosystem-convention violation (repos/UNS tokens are kebab, per the org
  AGENTS.md and every sibling repo), and every dogfooded scaffold had to hand-fix it. The Greengrass
  component name (`COMPONENTFULLNAME`) stays PascalCase reverse-DNS — only crate/bin/dir/artifact
  tokens change.
- **Consequence:** golden expectations in CLI tests and `cli.yml` change (`/tmp/scaffold/Gated` →
  `/tmp/scaffold/gated`); anyone re-scaffolding an existing component sees new names.

### SD-4 — Governance files: which templates, and whose LICENSE? (finding #10)

`AGENTS.md` + `CLAUDE.md` (importing it) + `DESIGN.md` skeletons are cheap and universally useful —
**recommendation: all 16 templates**. LICENSE is different: the sibling reference components are
**BUSL-1.1**, but a scaffold is the *author's* component — baking the EdgeCommons license choice
into third-party code is wrong, and today's templates already quietly stamp
`license = "Apache-2.0"` into `Cargo.toml` (its own small defect).
**Recommendation:** a new `--license <SPDX-id|none>` flag (default **`none`**), with embedded texts
for `BUSL-1.1`, `Apache-2.0`, `MIT`; when `none`, no LICENSE file and no `license =` manifest claim
(use `license` absent / `UNLICENSED` per language convention). Internal scaffolds pass
`--license BUSL-1.1`. **Consequence:** internal components keep exact parity; external users are not
mis-licensed by default.

### SD-5 — Should `pinned-rev` become the default `--dep-source`? (finding #3)

`pinned-rev` (rev in the manifest + emitted gitignored local override) is what every shipping sibling
actually uses, and it pins the **exact commit the embedded templates were authored against** — the
strongest correctness property available (the `registry` tag can lag the facades the template calls,
which is precisely what bit the dogfooding run).
**Recommendation: keep `local` as the default** (the monorepo-developer common case, and the current
documented default) but make `pinned-rev` the documented choice for "a real component repo", put it
in the wizard's dep-source prompt, and add it to the CI scaffold→build matrix (RUST + PYTHON legs).
**Consequence of flipping instead:** every casual `component new` inside the monorepo would fetch
the git dep unless the override path resolves — more surprising than helpful.

### SD-6 (minor) — Lockfile policy (finding #12)

A template cannot ship a *valid* `Cargo.lock`/`package-lock.json` (the graph depends on dep-source
and the resolution moment), and generating one at scaffold time requires toolchain + network —
violating the offline principle (DESIGN-cli P2). **Recommendation:** keep scaffolds lockfile-free;
add a **"Next steps" epilogue** to `component new` output (commit the lockfile after first build,
set the bucket, etc.), say it in the README and docs, and have `component validate` emit a
**warning** when a Rust/TS component has no committed lockfile. An opt-in `component new --lock`
(shell out to `cargo generate-lockfile` / `npm i --package-lock-only` when the toolchain exists) can
be a later add-on; not in this scope.

---

## The 12 findings as testable requirements

CLI/engine (Section A):

- **R1 (DEF-13).** Given `-n com.mbreissi.edgecommons.EthernetIpAdapter -l RUST`, the derived
  crate/bin/artifact name is `ethernet-ip-adapter` (case-boundary → kebab, acronym-aware), and
  `--bin-name`/`--crate-name` overrides it. Test: unit tests on the algorithm + a scaffold asserting
  `Cargo.toml [package].name`/`[[bin]].name`, Dockerfile, recipe artifact, supervisor conf,
  compose service all agree.
- **R2 (DEF-14).** The default output directory is the kebab name (`./ethernet-ip-adapter`), and
  `--dir <path>` sets it exactly. Test: scaffold and assert the directory.
- **R3 (DEF-15).** `--dep-source pinned-rev` (RUST, PYTHON) emits a git **rev**-pinned dependency in
  the manifest **and** a gitignored `.cargo/config.toml` `[patch]` sibling override (Rust). The rev
  is the workspace commit the CLI was built from (`EC_LIBRARY_REV`), overridable with
  `--library-rev`. `component upgrade` can parse and move the rev form. JAVA/TYPESCRIPT reject
  `pinned-rev` with a usage error naming why. Tests: `library_dep` forms, `.cargo` emission +
  gitignore, upgrade round-trip, CI scaffold→build legs.
- **R4 (DEF-16).** A Greengrass scaffold with no bucket writes a visible sentinel
  (`edgecommons-set-artifact-bucket`) into `gdk-config.json`, keeps the EC4005 warning, prints a
  "Next steps" line, and `component validate` **errors** on the sentinel. Test: scaffold without
  `-b`, run validate, assert the error code.

Template parity (Section B):

- **R5.** The 4 protocol-adapter templates emit `southbound_health` with the **exact §5 measure
  set** and ship the operational-family pattern per SD-1. Test: template unit tests assert measure
  names against a literal copy of the §5 list.
- **R6.** The 4 protocol-adapter templates register the full generic `sb/*` family (per SD-1/SD-2)
  with §2.2 request/reply shapes (batch `writes[]`, per-entry confirmation, allow-list checked
  **before** device I/O), D-EIP-13 instance routing, and the standardized error codes. Test:
  per-verb template tests in each language.
- **R7.** Every template ships a Diátaxis `docs/` set (adapters: + `reference/{configuration,
  messaging-interface,metrics,data-types}.md`) describing the **generated component as it is**,
  present tense, no roadmap markers. Test: files exist post-scaffold, tokens substituted, no
  `<<…>>` survivors (existing gate covers this once files carry tokens).
- **R8.** Every template ships `.github/workflows/ci.yml` calling
  `edgecommons/.github/.github/workflows/component-ci.yml@main` with the right `language`, plus a
  language-appropriate 90% coverage job, plus `deploy-docs.yml` (hook-guarded no-op). Test: files
  exist; YAML parses; language input matches the template language.
- **R9.** The 4 adapter templates ship a **simulator-gated, self-skipping** integration-test layout
  (`tests/` live suite gated on an env var) that is skipped in the scaffold-build gate and passes
  when pointed at a sim. Test: scaffold + run tests without the env var → skipped, suite green.
- **R10.** Every template ships `AGENTS.md`, `CLAUDE.md` (one-line `@AGENTS.md` import), and a
  `DESIGN.md` skeleton; LICENSE per SD-4. Test: files exist, tokens substituted.
- **R11.** The 4 adapter templates register the three edge-console panels (`overview`, `signals`,
  `diagnostics`; order 10/20/30; `scope: "instance"`) bound to the verbs the template actually
  registers. Test: a template test asserting `register_panel` was called with the three ids.
- **R12.** Per SD-6: `component new` prints a lockfile "next step" for Rust/TS; `component
  validate` warns on a missing committed lockfile; the README documents it. Test: validate warning.

---

## Section A — CLI/engine changes (findings #1–#4)

### A.1 The naming algorithm (R1) — kebab from mixed case, precisely

Replace the body of `ec-scaffold/src/generate.rs::bin_name()` (keep the name and signature — every
call site and the `BINNAME` token flow through it):

```text
kebab(short):
  classify each char: U = ASCII uppercase, L = ASCII lowercase, D = ASCII digit,
                      S = anything else (separator, dropped)
  a word boundary exists between adjacent kept chars c1, c2 when:
    (a) class(c1) ∈ {L, D} and class(c2) = U          # lower/digit → Upper: "netIp" → net|Ip
    (b) class(c1) = U and class(c2) = U and the char after c2 has class L
                                                      # acronym end: "OPCUAAdapter" → OPCUA|Adapter
  S chars are boundaries themselves (and are dropped).
  lowercase everything; join words with '-'; collapse consecutive '-'; trim leading/trailing '-'.
  empty result → "component".
```

Worked examples (these become the unit-test table):

| Input (short name) | Output |
|---|---|
| `MyComponent` | `my-component` |
| `EthernetIpAdapter` | `ethernet-ip-adapter` |
| `OPCUAAdapter` | `opcua-adapter` |
| `ModbusTCPAdapter` | `modbus-tcp-adapter` |
| `Modbus2Tcp` | `modbus2-tcp` |
| `My_Cool.Component` | `my-cool-component` |
| `mycomponent` | `mycomponent` |
| `___` | `component` |

Rule (b) is the acronym rule: in an uppercase run followed by a lowercase letter, the **last**
uppercase letter starts the next word (`HTTPServer` → `http-server`, `EthernetIP` → `ethernet-ip`).
Digits glue to the preceding word (`Modbus2` stays one word) and a digit→Upper transition is a
boundary via rule (a).

**New tokens and flags:**

| Change | Detail |
|---|---|
| `BINNAME` token | Now `kebab(short)`; overridden by the new flag. Single token, so **every** consumer (Cargo.toml, Dockerfile, recipe.yaml artifact, build.sh, supervisor conf, compose service, k8s names, test-configs) moves consistently — the exact set of files the dogfooding had to hand-fix. |
| **New** `SNAKENAME` token | `BINNAME` with `-` → `_`. Used to rename the Python package dir to the `modbus_adapter`-style module name (the python templates' `app/` dir becomes `{SNAKENAME}/` via a manifest `renames` entry) and for the pyproject `[project] name`. |
| `JARNAME` token | Redefined as `BINNAME` (Maven artifactId/finalName convention is lower-kebab; the shaded-jar name in `recipe.yaml` follows). `MAINCLASSNAME`/`PACKAGE`/`PACKAGEPATH` unchanged — the Java *class* stays PascalCase. |
| **New flag** `--bin-name <name>` (visible alias `--crate-name`) | Validated `^[a-z0-9][a-z0-9-]*$`; sets `BINNAME` (and therefore `SNAKENAME`/`JARNAME`). One flag for all languages — a per-language flag pair would be parity drift. |

**Parity story per language** (finding #1 asks this explicitly): Rust — crate + `[[bin]]`; Java —
Maven `artifactId` + shaded jar + recipe artifact name (the Greengrass *component name* remains
`COMPONENTFULLNAME`, PascalCase reverse-DNS, per the org convention "component name PascalCase;
crate/bin/artifact and UNS token kebab"); Python — distribution name `edgecommons-<BINNAME>`-style
stays as the template's pyproject pattern with `BINNAME`, module dir `SNAKENAME`; TypeScript —
`package.json` `"name"` = `BINNAME`, built binary/entry unchanged.

### A.2 Output directory (R2)

In `component.rs`: `let target = args.dir.clone().unwrap_or_else(|| args.path.join(bin_name(&short_name(&full_name))))`.

- **New flag** `--dir <path>`: the exact output directory (conflicts with nothing; `-p/--path`
  remains the parent for the derived default). No `--repo-name` — `--dir` subsumes it.
- The "Generating …" and "Done. Component generated at:" lines already print the target; no change.

### A.3 `--dep-source pinned-rev` (R3)

**Data model.** `DepSource` gains a third variant in both `ec-scaffold` and the clap enum:

```rust
pub enum DepSource { Local, Registry, PinnedRev }   // as_str: "local" | "registry" | "pinned-rev"
```

`flags()` already emits `dep:{as_str}`, so manifests can gate on `dep:pinned-rev` with **no
manifest-schema change** (`conditional` is the existing mechanism; `deny_unknown_fields` is
untouched).

**Where the rev comes from.** Extend `ec-scaffold/build.rs` (the DEF-1 pattern, deliberately): run
`git rev-parse HEAD` in the repo root at CLI build time → `cargo:rustc-env=EC_LIBRARY_REV=<sha>`;
`rerun-if-changed=.git/HEAD`. If git is unavailable (tarball build), emit an empty value — then a
runtime `--dep-source pinned-rev` without `--library-rev` is a `Fatal::Environment` error naming the
fix. Rationale: **the embedded templates and the rev come from the same commit**, so the pinned rev
by construction contains every facade the template calls — the exact failure `registry`'s lagging
tag produced in the dogfooding. No network at scaffold time (P2 preserved). **New flag**
`--library-rev <sha>` overrides (also the escape hatch for "pin to current origin/main yourself").

**The dependency table** (extends DESIGN-cli §7.1; `library_dep()` remains the single source):

| Language | `pinned-rev` emission | Local-dev override emitted |
|---|---|---|
| Rust | `git = "https://github.com/edgecommons/edgecommons", rev = "<sha>"` | `.cargo/config.toml` with `[patch."…/edgecommons"] edgecommons = { path = "<LIBRARY_LOCAL_PATH>" }` + `[net] git-fetch-with-cli = true` (verbatim the ethernet-ip-adapter shape) |
| Python | `edgecommons @ git+https://github.com/edgecommons/edgecommons@<sha>#subdirectory=libs/python` | none (README documents `pip install -e <sibling>` as the local override) |
| Java | **usage error**: Maven cannot express a git dependency; message points to `registry`/`local` | — |
| TypeScript | **usage error**: npm git deps cannot address the `libs/ts` subdirectory of the monorepo | — |

The Java/TS rejection happens in `component.rs` input resolution, **before** generation, with the
reason in the message. This is an honest capability limit, not a silent fallback.

**How `.cargo/config.toml` is emitted: a template file, not engine synthesis.** The rust templates
gain `.cargo/config.toml` containing the `<<LIBRARY_LOCAL_PATH>>` token, gated by
`"conditional": [{ "when": "dep:pinned-rev", "paths": [".cargo"] }]`. Engine synthesis would special-
case one language inside `generate()`; a conditional-gated file is the mechanism the engine already
has, stays visible in `template show`, and needs zero engine code. **New token
`LIBRARY_LOCAL_PATH`**: resolved like the `Local` library path (`--library-path`, else
`repo_root().join("libs/rust")`), POSIX-slashed; if the path does not exist on this machine the file
is still emitted (it is gitignored dev tooling) with a comment saying to fix the path. The rust
templates' `.gitignore` gains an unconditional `/.cargo/` line with the "LOCAL DEV ONLY" comment
(harmless when the dir is absent; `.gitignore` cannot be conditionally assembled by the engine).

**`component upgrade` (DESIGN-cli §7.1 contract extension).** `bump_cargo` learns the `rev = …`
inline-table form: `upgrade --to X.Y.Z` on a rev-pinned dep rewrites it to the tag form
(`tag = "rust-lib/vX.Y.Z"`, removing `rev`) and says so in the change description ("rev pin →
release pin"); a **new** `component upgrade --to-rev <sha>` (mutually exclusive with `--to`) moves
the rev in `Cargo.toml` and the `@<sha>` in a git-pinned `requirements.txt` line, and is a usage
error for Java/TS projects. `--dry-run` covers both.

### A.4 The empty publish bucket (R4)

Generation change in `component.rs`: when the GREENGRASS pack is selected and `bucket` resolves
empty, substitute the sentinel **`edgecommons-set-artifact-bucket`** for `BUCKET` instead of the
empty string (the sentinel is a plain literal — it does not trip the `<<TOKEN>>` gate). Keep EC4005.
Add a **"Next steps"** epilogue after the "Done." line (non-quiet mode) listing: set the bucket (if
sentinel), build once and commit the lockfile (Rust/TS), wire the repo into org CI secrets. New
artifact-lint rule in `ec-validate` (EC3xxx range): `gdk-config.json` containing the sentinel is an
**error** from `component validate` — so the miss is caught at authoring or CI, not at
`gdk component publish` weeks later.

### A.5 Tests that change / new tests (Section A)

Existing tests that must change:
- `generate.rs::bin_names_are_cargo_safe` — expectations become kebab (`my-component`,
  `my-cool-component`); add the full A.1 example table.
- `generate.rs` tests that join `dir.path().join("MyComponent")` — target-dir derivation moves into
  `component.rs`; add a `component.rs` test that the default dir is kebab and `--dir` wins.
- `registry_dep_pins_the_real_current_version` — unchanged, plus new
  `pinned_rev_dep_pins_the_build_rev` and `pinned_rev_without_a_rev_is_an_environment_error`.
- `cli.rs::noun_verb_parses` family — add parses for `--bin-name`, `--dir`, `--library-rev`,
  `--dep-source pinned-rev`, `upgrade --to-rev`.
- `upgrade.rs` — new: `rust_rev_dependency_is_bumped_to_tag_by_to`,
  `rust_rev_is_moved_by_to_rev`, `python_git_rev_pin_is_moved_by_to_rev`,
  `to_rev_on_java_is_a_usage_error`.
- `no_unsubstituted_token_survives_any_template` — unchanged but now sweeps the new files; it is the
  gate that keeps every Section-B addition honest.
- `cli.yml` scaffold-build matrix: names change per SD-3 (`/tmp/scaffold/gated`); **add** two legs
  `RUST + --dep-source pinned-rev` and `PYTHON + --dep-source pinned-rev` (network fetch of the
  pinned rev is fine in CI; it proves the emitted pin resolves), and keep the default-local legs.
- New: `.cargo` emission test (present under `pinned-rev`, absent under `local`/`registry`);
  bucket-sentinel + validate-error test; "next steps" epilogue snapshot test.

Coverage: all new engine code lands under the existing 90% CLI gate.

---

## Section B — Template-parity changes (findings #5–#12)

Applicability matrix (16 templates = {java, python, rust, typescript} × {service, protocol-adapter,
processor, sink}):

| Gap | service | protocol-adapter | processor | sink |
|---|---|---|---|---|
| #5 metrics families | — | **all 4 langs** | — | — |
| #6 `sb/*` commands | — | **all 4 langs** | — | — |
| #7 Diátaxis docs | all 4 | all 4 (+ full reference set) | all 4 | all 4 |
| #8 CI workflows | all 4 | all 4 | all 4 | all 4 |
| #9 integration tests | — | **all 4 langs** | — | — |
| #10 governance files | all 4 | all 4 | all 4 | all 4 |
| #11 console panels | — | **all 4 langs** | — | — |
| #12 lockfile policy | Rust+TS | Rust+TS | Rust+TS | Rust+TS |

Where a gap is adapter-specific it is closed **in all four adapter templates** (parity rule): the
Rust adapter template is the richest today (device seam + supervisor + backoff); the Java template
is a single `App.java` with no seam, Python is `app/adapter.py`, TS has `app.ts`/`device.ts`. Gap
#6 therefore includes **introducing the `DeviceSession`/`DeviceBackend` seam in Java and Python**
(`Device.java` interface pair + sim; `device.py` seam + sim, modbus-style) so the four adapters
teach the same shape — this is the largest single work item in Section B and is called out in the
plan as its own [OPUS] items.

### B.5 Metrics (R5, per SD-1 Option A)

Files per language (adapter templates): Rust `src/metrics.rs` (new module; `app.rs` slims to use
it), Java `Metrics.java`, Python `{SNAKENAME}/metrics.py`, TS `src/metrics.ts`. Content, identically
shaped across languages:

1. `southbound_health`, dimensioned by `instance`, with **exactly** the §5 canonical measures:
   `connectionState` (Count/1), `publishLatencyMs` (Ms/1), `pollLatencyMs` (Ms/1), `readErrors`
   (Count/60), `staleSignals` (Count/60), plus the §5-sanctioned optional `reconnects` (Count/60).
   `staleSignals` needs a per-signal last-update tracker driven by a new
   `component.global.healthThresholds.staleSignalSecs` config key (added to each adapter
   `config.schema.json` and `test-configs`, default 30 — the §4 convention key).
2. The **operational-family pattern**: an interval/total counter pair type (modbus `_Counter`
   equivalent), two worked families `<<COMPONENTNAME>>Connection` (`connectionState`,
   `connectAttempts`, `connectFailures`, `reconnectAttempts`, `connectionDrops`,
   `connectedDurationMs`) and `<<COMPONENTNAME>>Command` (`commandRequests`, `commandLatencyMs`,
   `commandErrors`, dimensioned `instance`×`verb`×`result` with the low-cardinality rule stated in
   a doc comment), an emit loop on the metrics interval, and a signposted "add `Inventory` / `Poll`
   / `Publish` families for your protocol here — see modbus-adapter/ethernet-ip-adapter" extension
   point.
3. Template tests: measure names asserted against literal §5 lists; interval counters reset on emit;
   dimensions are low-cardinality only.

### B.6 Command surface (R6, per SD-1/SD-2)

Adapter templates, all 4 languages, one `commands` module each (Rust `src/commands.rs`, Java
`Commands.java`, Python `{SNAKENAME}/command_service.py`, TS `src/commands.ts`) registering on the
`commands()` inbox (the shipped 4-language facade — this matches what modbus/EIP actually do; the
SOUTHBOUND §2.2 "Phase 5 not built" note refers to a *core-owned* facade, which templates do not
need):

| Verb | Behavior in the template (against the sim seam) |
|---|---|
| `sb/status` | Per-instance link state/paused/endpoint from the same `Health` the connectivity provider reads (one source, two surfaces). |
| `sb/read` | On-demand read of named signals through the seam; §2.2 reply shape `{id, reads:[{signal,value,quality,qualityRaw,…}]}`. Seam gains `read_named(ids)` (or reads-all-and-filters in the sim). |
| `sb/write` | **§2.2 batch shape**: `{writes:[{signalId, value}, …]}` (single object also accepted), allow-list checked per entry **before any device I/O**, per-entry confirmed result. Replaces today's single-write handler and its off-contract `{instance, signalId, value}` flat shape. |
| `sb/signals` | The configured signal inventory (from config + sim), no device round-trip. |
| `sb/browse` | New optional seam method (`browse(page)`); the sim implements a one-page browse; the default trait/interface impl returns `BROWSE_UNSUPPORTED`, so a protocol without discovery stays honest. |
| `reconnect` | Sends a control message to the device task (drop session → connect loop). |
| `repoll` | Triggers an immediate poll cycle. |
| `sb/pause` / `sb/resume` | **Per SD-2 — excluded pending the SOUTHBOUND promotion decision.** |

Shared conventions, stated in each module's doc header: D-EIP-13 instance routing (`body.instance`
optional iff exactly one configured instance, else `BAD_ARGS`); the standardized error codes
(`BAD_ARGS`, `NO_SUCH_INSTANCE`, `WRITE_NOT_ALLOWED`, `WRITE_FAILED`, `DEVICE_UNAVAILABLE`,
`READ_FAILED`, `RECONNECT_FAILED`, `BROWSE_UNSUPPORTED`, `BROWSE_FAILED`); session-touching verbs
ride the device task's control channel and are confirmed. Each verb records into the
`<<COMPONENTNAME>>Command` family (B.5). Per-verb template tests in each language.

### B.7 Diátaxis docs (R7) — all 16 templates

Per template: `docs/README.md` (nav page), `docs/tutorial.md` (scaffold → run against the sim →
observe on the UNS), `docs/how-to-guides.md` (replace the sim with a real backend/stage/destination;
configure platforms; wire CI), `docs/explanation.md` (the archetype's shape: the seam, the
supervisor, quality semantics — adapted per kind), `docs/sample-configurations.md` (the shipped
`test-configs` explained + one non-trivial variant). Adapters add `docs/reference/{configuration.md,
messaging-interface.md, metrics.md, data-types.md}` (modeled file-for-file on
`modbus-adapter/docs/reference/`); service/processor/sink add `docs/reference/{configuration.md,
messaging-interface.md, metrics.md}` (no data-types — that page is the southbound value-mapping
table, adapter-specific). Rules: **describe the generated component as it currently is** (the sim,
the actual verbs/metrics), plain present tense, no roadmap/status markers (org public-docs rule);
`<<COMPONENTNAME>>`/`<<BINNAME>>` substituted; no frontmatter (site-sync convention). These are
teaching prose seeded from real behavior — not lorem stubs — but they intentionally document the
*archetype*, and each page opens with a one-line "this documents the scaffold; rewrite as you build"
marker the author deletes.

Docs-site implication: the sync script pulls `docs/` only from **registered** components, so
scaffold docs reach the site exactly when a component is added to `registry/components.json` — no
site change needed. Ship `deploy-docs.yml` (below) so registered components refresh the site on
doc-only pushes, as modbus does.

### B.8 CI (R8) — all 16 templates

Two workflow files per template, tokens `<<COMPONENTNAME>>` only where needed:

1. `.github/workflows/ci.yml` — verbatim the modbus/EIP caller shape: `paths-ignore` for docs,
   `uses: edgecommons/.github/.github/workflows/component-ci.yml@main`,
   `with: { language: <LANG> }`, `secrets: inherit`, `permissions: contents: read` — **plus** a
   `coverage` job per language, since the reusable workflow does build/test/clippy only:
   Rust `cargo llvm-cov --fail-under-lines 90`; Python
   `pytest --cov=<<SNAKENAME>> --cov-fail-under=90`; TypeScript `npm run coverage` (vitest
   thresholds set in the template config); Java `mvn verify` with a JaCoCo 90% check added to the
   template `pom.xml`. A comment names the org rule (90% line, live-infra excluded, don't lower).
2. `.github/workflows/deploy-docs.yml` — the hook-guarded no-op docs-rebuild trigger, verbatim from
   modbus.

These files are inert until the repo is pushed to GitHub with the org secrets — harmless locally,
and their absence was a per-repo re-derivation cost the dogfooding paid.

### B.9 Integration-test layout (R9) — adapter templates, all 4 languages

A `tests/` live suite skeleton, **self-skipping** unless an env var (`EC_LIVE_SIM=<endpoint>`)
points at a simulator: Rust `tests/live_sim.rs` (early-return + `eprintln!("skipped: …")` when
unset, matching the EIP/file-replicator gating idiom); Python `tests/test_live_sim.py`
(`pytest.mark.skipif`); Java `LiveSimIT.java` (`@EnabledIfEnvironmentVariable`); TS
`test/live-sim.test.ts` (`describe.skipIf`). Content: connect → one poll cycle → assert readings +
quality; a comment shows how the sibling adapters run theirs (cpppo/OpENer, the permanent modbus sim
container). The scaffold-build CI gate stays green because the suite self-skips.

### B.10 Governance files (R10, per SD-4) — all 16 templates

- `AGENTS.md`: the component's shape (name tokens, what it is, the seam, config location, validation
  expectations, the org conventions it inherits) — modeled on the sibling repos' AGENTS.md but
  scaffold-scoped; tokens substituted.
- `CLAUDE.md`: the sibling pattern — a short header + `@AGENTS.md` import + a local-dev section
  (dep-source-aware: mentions the `.cargo` override when `pinned-rev`, via the same conditional
  mechanism? **No** — one file cannot be conditionally assembled; the text covers both cases in two
  short bullets).
- `DESIGN.md`: a skeleton with the section headers the ecosystem uses (What it is / Decisions
  D-XXX-1… / Config / Command surface / Metrics / Validation) and a one-paragraph instruction to
  treat it as the design-fidelity contract.
- `LICENSE`: only when `--license` is passed (SD-4); the flag also sets the manifest license fields
  (`Cargo.toml license`, `package.json license`, `pyproject license`, `pom.xml <licenses>`) —
  and when `none`, the current hardcoded `Apache-2.0` strings are **removed** from the templates.

### B.11 Edge-console panels (R11) — adapter templates, all 4 languages

Port the ethernet-ip-adapter `panels()` trio, bound to the template's actual verb set (B.6):
`overview` (summary of `connected/state/endpoint` + command summary `reconnect`), `signals`
(signalGrid; verbs `sb/signals`, `sb/read`, `sb/write`, `repoll`), `diagnostics` (treeBrowser +
keyValueList; verbs `sb/browse`, `sb/status`); order 10/20/30, `scope: "instance"`; registered via
`commands.register_panel` (verified present in all four libraries). If SD-2 later admits
pause/resume, `overview` regains those actions in the same change.

### B.12 Lockfile (R12, per SD-6)

No embedded lockfiles. Work items: the "Next steps" epilogue line (A.4) for Rust/TS scaffolds; a
README "first build → commit the lockfile" note (the rust template's `.gitignore` already carries
the right comment — mirror it in TS, whose `.gitignore` must not ignore `package-lock.json`); the
`component validate` **warning** for a missing committed lockfile in Rust/TS projects.

### Where the "minimal archetype" line now sits (recommendation)

After this work a template is: **runnable against its sim, canonical on the published contracts
(§5 metrics, §2.2 command shapes), complete on repo hygiene (docs/CI/governance/tests/panels), and
deliberately generic on protocol specifics** (no protocol families beyond the two worked ones, no
real backend, one-page sim browse). That is the line between "minimal archetype" and the reference
adapters, stated so it can be enforced in review rather than re-litigated per template.

---

## Cross-cutting

- **`template list` / `template show`:** no code change — both are discovery-driven and pick up new
  files/conditionals automatically. Template `description` strings are refreshed to mention the
  parity surface (they show in `template list`).
- **`component validate`:** two new artifact-lint rules (bucket sentinel = error; missing committed
  lockfile Rust/TS = warning), codes allocated in the EC3xxx/EC4xxx ranges in `ec-diag`.
- **DESIGN-cli.md updates (same change as the code, per the doc-sync rule):** §5.7 token set gains
  `SNAKENAME`, `LIBRARY_LOCAL_PATH`, the `BINNAME` kebab definition, and the `--bin-name`/`--dir`
  flags; §7.1 table gains the `pinned-rev` row + `--to-rev`; §5.1's historical matrix gets a
  current-state note (16/16 templates exist); §12 gains DEF-13…DEF-16 (findings #1–#4, evidence:
  CLI-DOGFOODING.md); §13 gains D-CLI-17 (kebab naming + overrides; GG component name stays
  PascalCase), D-CLI-18 (pinned-rev from the CLI's build commit + emitted gitignored override; no
  network at scaffold time), D-CLI-19 (the archetype-parity floor as defined above), D-CLI-20
  (bucket sentinel + validate error), D-CLI-21 (license is the author's choice; `--license` flag,
  default none), D-CLI-22 (lockfiles: instruct-and-validate, never embed), D-CLI-23 (pause/resume
  excluded from templates pending SOUTHBOUND promotion — or the inverse, per SD-2).
- **Other stale docs fixed in the same change:** `core/CLAUDE.md` "Two axes … Today: …" template
  list (claims TS is service-only — the tree is 16/16); SOUTHBOUND.md §6 template description
  updated to the new archetype surface (present tense, no history); the website's CLI/scaffolding
  pages (`core/website`) re-checked against the new flag surface and template matrix.
- **Definition of done (org contract):** scaffold→**build→test** green for all 16 templates ×
  {local, registry} + {rust, python} × pinned-rev, in `cli.yml`; CLI crates ≥ 90% coverage; one
  scaffolded adapter (Rust) run as a HOST smoke against EMQX to prove the runtime path; docs
  (DESIGN-cli, core/CLAUDE.md, SOUTHBOUND §6, website) updated **wholesale** in the same change;
  stale claims replaced, not annotated. Greengrass/K8s deploy validation is **not** triggered — no
  core-library or wire behavior changes here (templates emit the same shipped facades) — but if
  any template `recipe.yaml` change alters GG lifecycle behavior, a lab-5950x scaffold-deploy smoke
  is added to the phase that changes it.

## Design-fidelity notes (deviations surfaced, not decided)

1. **Full literal metric/command parity with the reference adapters is impossible in a generic
   template** (protocol-named families, protocol browse). SD-1 states the honest middle ground;
   choosing Option A is a *scoping decision you make*, not a silent reduction.
2. **`sb/pause`/`sb/resume`** (SD-2): including them promotes a single-adapter extension to
   ecosystem contract without a SOUTHBOUND.md change; excluding them leaves the dogfooded adapter
   richer than the template. Neither is done silently.
3. **`pinned-rev` for Java/TypeScript is rejected, not emulated** — Maven and npm cannot express the
   monorepo-subdirectory git pin. This is a stated capability limit (A.3), mirroring the sibling
   convention which only exists for Rust/Python-style git deps.
4. **Lockfiles are not embedded** (SD-6) — embedding would fake determinism the scaffold cannot
   have offline.
5. **The template `Apache-2.0` stamps are removed when `--license` is absent** — a behavior change
   to current output, folded into SD-4.

---

## Phased Implementation Plan

Engine changes land before any template that depends on new tokens/flags/conditionals. Every phase
ends with `cargo test` in `cli/` + the `no_unsubstituted_token_survives_any_template` sweep; phases
3–6 also run the affected `cli.yml` scaffold-build legs locally.

**Phase 0 — Sign-off.** User decides SD-1…SD-6. No code.

**Phase 1 — Engine & CLI (findings #1–#4).**
- 1.1 **[OPUS]** (engine; all languages) Naming: `bin_name()` kebab algorithm + example-table tests;
  `SNAKENAME`/`JARNAME` token changes; `--bin-name`/`--crate-name` flag + validation; default
  output dir → kebab + `--dir` flag; update every existing test/golden that assumed the old names.
- 1.2 **[OPUS]** (engine; Rust+Python emission, Java/TS rejection) `DepSource::PinnedRev`:
  `build.rs` `EC_LIBRARY_REV`; `library_dep()` rev forms; `--library-rev`; Java/TS usage errors;
  `LIBRARY_LOCAL_PATH` token; `upgrade` rev-form parsing + `--to-rev`; full test set per A.5.
- 1.3 **[SONNET]** (rust templates only) The `.cargo/config.toml` conditional file + `.gitignore`
  lines in `rust`, `rust-protocol-adapter`, `rust-processor`, `rust-sink` manifests.
- 1.4 **[SONNET]** (all GG-capable templates) Bucket sentinel substitution + "Next steps" epilogue;
  **[OPUS]** the new `ec-validate` sentinel-error + lockfile-warning rules (diagnostic codes, rule
  wiring, tests).
- 1.5 **[SONNET]** `cli.yml`: renamed target dirs; the two pinned-rev matrix legs.

**Phase 2 — Rust adapter template uplift (the archetype reference).**
- 2.1 **[OPUS]** (rust-protocol-adapter) Metrics per B.5: canonical `southbound_health` +
  staleness tracker + the two worked operational families; config.schema.json + test-configs
  additions; template tests.
- 2.2 **[OPUS]** (rust-protocol-adapter) Commands per B.6: seam extensions (`browse`, named reads,
  control channel verbs), the seven verbs, §2.2 shapes, error codes, instance routing; panels per
  B.11; template tests per verb + panel-registration test.

**Phase 3 — Cross-language adapter parity (port Phase 2).**
- 3.1 **[OPUS]** (java-protocol-adapter) Introduce the Device seam (interfaces + sim) and port
  metrics/commands/panels/tests. The Java template is furthest from the shape — largest port.
- 3.2 **[OPUS]** (python-protocol-adapter) Seam + `{SNAKENAME}/` module rename (uses the Phase-1
  token) + metrics/commands/panels/tests, modeled on modbus-adapter.
- 3.3 **[OPUS]** (typescript-protocol-adapter) Extend the existing device.ts seam; port
  metrics/commands/panels/tests.
- Each of 3.1–3.3 must build+test via its `cli.yml` leg before the phase closes; behavior must match
  2.x (same verbs, same error codes, same measure names) — reviewed against B.5/B.6, not summaries.

**Phase 4 — Universal packs (findings #7, #8, #10, #12), all 16 templates.**
- 4.1 **[SONNET]** (×16) CI workflows per B.8 (incl. the Java JaCoCo/TS vitest threshold additions
  to the build manifests) + deploy-docs.yml.
- 4.2 **[SONNET]** (×16) Governance files per B.10 + the `--license` flag **[OPUS for the flag
  itself** — embedded texts, manifest-field wiring across four manifest formats — **SONNET for the
  per-template files]**.
- 4.3 **[SONNET]** (×16) Diátaxis docs per B.7 — content seeded from each template's real behavior;
  adapters get the full reference set. (Large but mechanical once the adapter reference set is
  written once per kind; the per-language deltas are the command/metric tables from Phases 2–3.)
- 4.4 **[SONNET]** (adapters ×4) Integration-test layouts per B.9.
- 4.5 **[SONNET]** Manifest `substitutions` entries for every added tokenized file (the no-token
  gate enforces completeness).

**Phase 5 — Docs & registers (same-change rule applies within each phase above for the docs those
phases own; this phase is the cross-repo sweep).**
- 5.1 **[SONNET]** DESIGN-cli.md: §5.7/§7.1 updates, DEF-13…16, D-CLI-17…23 (final numbering per
  SD outcomes).
- 5.2 **[SONNET]** core/CLAUDE.md template-matrix fix; SOUTHBOUND.md §6 refresh; website CLI pages.

**Phase 6 — Validation & closure.**
- 6.1 **[SONNET]** Full `cli.yml` matrix green (16 × local + registry legs, + 2 pinned-rev legs);
  CLI coverage ≥ 90%.
- 6.2 **[OPUS]** HOST smoke: scaffold the Rust adapter with `pinned-rev`, build, run against local
  EMQX, observe `data`/`state`/`metric` on the UNS wildcards and exercise `sb/status`/`sb/write`
  round-trip. This is the "does the scaffold actually run" gate the dogfooding implicitly ran.
- 6.3 **[SONNET]** Update `ethernet-ip-adapter/CLI-DOGFOODING.md` with a closure note per finding
  (internal dev note — history is allowed there), and re-verify no stale claims remain in the
  touched docs.

Dependency order: 1.1/1.2 → 1.3–1.5; Phase 2 → Phase 3; Phase 1 tokens → 3.2 rename and 4.x
tokenized files; Phases 2–3 → 4.3 (docs describe the uplifted behavior). Phases 3.1–3.3 can run in
parallel; 4.1–4.4 can run in parallel per template family.
