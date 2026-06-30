# The edgecommons ecosystem

> Status: **plan + staging** (2026-06-29). The GitHub org does not exist yet; this document is the
> blueprint and `ecosystem/staging/` holds the content that will seed it. Nothing here changes the
> `ggcommons` library itself.

## What this is

`ggcommons` is the library/CLI/templates core (this monorepo). On top of it sits a growing family of
**components** — protocol adapters, edge processors, northbound sinks — that are *not* part of this
repo and live in their own repositories (the OPC UA and Modbus reference adapters already do). This
document defines how those repositories are organized and associated on GitHub, and the plan to get
there.

The seed components:

| Component | Lang | Repo (target) | What it is |
|-----------|------|---------------|-----------|
| `opcua-adapter` | Java | `edgecommons/opcua-adapter` | Southbound OPC UA adapter (Eclipse Milo), subscribe-based. Republishes node changes as `SouthboundSignalUpdate`; serves reads/writes; emits `southbound_health`. |
| `modbus-adapter` | Python | `edgecommons/modbus-adapter` | Southbound Modbus adapter (pymodbus), poll-based. TCP / serial RTU / RTU-over-TCP. Same southbound contract as the OPC UA adapter. |
| `telemetry-processor` | Rust | `edgecommons/telemetry-processor` | Reference **processor**. Subscribes to local telemetry topics, runs a per-route filter/sample/aggregate/project/Rhai pipeline, and forwards to local / northbound MQTT / a durable stream (Kinesis / Kafka / rolling Parquet-AVRO files). See `docs/TELEMETRY_PROCESSOR.md`. |

Both implement the southbound contract in `docs/SOUTHBOUND.md` and run on GREENGRASS, HOST, or
KUBERNETES.

## Decision: a dedicated GitHub Organization, `edgecommons`

A GitHub **Organization** is the only GitHub-native primitive that *contains* a family of repos —
one landing page, org-scoped packages, shared teams, org Projects, and reusable CI. Associating
repos by topics/naming alone (on the personal `mbreissi` account) only glues them together by
convention and leaves them mixed with unrelated personal projects.

The org is named **`edgecommons`** — platform-neutral (the framework now targets Greengrass **and**
HOST/Docker **and** Kubernetes), while keeping the distinctive "commons" identity. Greengrass
discoverability is preserved via repo **topics** (`aws-iot-greengrass`), not the brand.

**The library keeps the name `ggcommons`.** Renaming it would churn every published coordinate
(`com.mbreissi:ggcommons`, PyPI `greengrass-commons`, the crate, npm, the CLI, the docs domain) for
no functional gain. The org name is deliberately broader than its flagship — a common, healthy
pattern. A future convergence rename (`ggcommons` → `edgecommons`) stays *possible* but is explicitly
out of scope here.

## Target structure

```
edgecommons (GitHub org)
├─ ggcommons        ← this monorepo, transferred from mbreissi/ggcommons (auto-redirects)
├─ .github          ← org profile README (front door) + shared health files
│                     + reusable CI/publish/deploy workflows
├─ registry         ← machine-readable component catalog (private; read via gh auth)
├─ opcua-adapter    ← from source/java/gg-opcua-adapter
├─ modbus-adapter   ← from source/python/gg-modbus-adapter
└─ …future components
```

### Naming & taxonomy

- **Flat repo names** — `opcua-adapter`, not `edgecommons-opcua-adapter`. The org already namespaces
  them (`edgecommons/opcua-adapter`).
- **Category is carried in topics + the registry, never in the name.** Names are painful to change;
  topics and registry entries are trivial to reorganize.
- Categories: **adapter** (southbound / field-device ingestion), **processor** (edge
  transform/aggregation/analytics), **sink** (northbound forwarding to cloud/historian/Kafka), plus
  **core** (the library, CLI, docs).
- Topics on every repo: `edgecommons`, `aws-iot-greengrass`, `iiot`, `edge-computing`, plus a type
  topic (e.g. `edgecommons-adapter`) and a protocol topic (`opc-ua`, `modbus`, …).

## The glue

1. **Org profile README** (`edgecommons/.github/profile/README.md`) — the human front door, listing
   the library + every component grouped by category. Staged in `ecosystem/staging/org-dotgithub/`.
2. **Default community health files** (`CONTRIBUTING`, `SECURITY`, `CODE_OF_CONDUCT`, issue/PR
   templates) in `edgecommons/.github` — inherited by every org repo that lacks its own.
3. **Reusable workflows** in `edgecommons/.github` — components call
   `uses: edgecommons/.github/.github/workflows/component-ci.yml@main`, so they all get identical
   build/test/publish CI with no copy-paste and central updates. (The doubled `.github/.github`
   path is the standard quirk of the special `.github` repo; move them to a dedicated
   `edgecommons/ci` repo if it bothers you.)
4. **Component registry** (`edgecommons/registry`) — a machine-readable catalog consumed by the CLI
   (`ggcommons list-components`, later `ggcommons add <name>`) and rendered as a "Components" page on
   the docs site. This is what makes the ecosystem *browsable and installable* rather than a folder
   of repos. Staged in `ecosystem/staging/registry/`.
5. **Convention** — new components are born via `ggcommons create-component`, pushed to the org under
   the naming rules, and added to the registry via a PR.

## The registry

`edgecommons/registry/components.json` (validated against `registry.schema.json`) is the source of
truth for "what components exist." Each entry:

```json
{
  "name": "opcua-adapter",
  "repo": "edgecommons/opcua-adapter",
  "language": "JAVA",
  "category": "adapter",
  "protocol": "OPC UA",
  "description": "…",
  "status": "beta",
  "platforms": ["GREENGRASS", "HOST", "KUBERNETES"],
  "library": "com.mbreissi:ggcommons",
  "topics": ["edgecommons", "aws-iot-greengrass", "iiot", "opc-ua", "edgecommons-adapter"]
}
```

The registry repo is **private** (matching the rest of the ecosystem). The CLI reads it with
authentication via `gh`; the docs site reads it with a token at build time. (Flip just this repo to
public later if you want tokenless reads — the CLI's URL/path override already supports that.)

**Consumers:**
- CLI: `ggcommons list-components [--language …] [--category …] [--json] [--source URL|path]`.
  Default: fetch the private catalog via `gh api` (authenticated). `--source`/`$GGCOMMONS_REGISTRY_URL`
  override with a URL or local path. Implemented in `cli/ggcommons_cli/commands/list_components.py`.
- Docs site: a build-time fetch of `components.json` → a "Components" reference page.

## Realization plan

> Operational step-by-step for Phases 0–1 (org + self-hosted runner setup, repo transfer, and the
> package-coordinate repointing) lives in **`ecosystem/RUNBOOK.md`**, with a dry-run-by-default
> script at `ecosystem/repoint-to-edgecommons.ps1` (and `.sh`).

### Phase 0 — Create the org *(manual, web UI)*
- Create `edgecommons` at `github.com/account/organizations/new` (Free plan to start; org creation
  is **not** available via `gh`/API).
- Set: new repos default **private**, enable GitHub Packages, set base member permissions.
- CI minutes: stay on **GitHub-hosted runners** and **accept the Free-org 2,000 private min/mo cap
  for now** (no self-hosted runner — local runners are intentionally avoided initially). Keep usage
  under the cap with path filters / lean matrices / caching (see `ecosystem/RUNBOOK.md` § Phase 0b),
  and add a self-hosted runner only later if the cap actually bites (§ Phase 0c).

### Phase 1 — Move the library + stand up shared infra
1. **Transfer** `mbreissi/ggcommons` → `edgecommons/ggcommons` (repo Settings → Transfer). GitHub
   redirects the old path indefinitely; existing remotes/clones/package refs keep working. Then
   `git remote set-url origin git@github.com:edgecommons/ggcommons.git`.
2. **Repoint package coordinates** (all internal consumers, so contained):
   - `ghcr.io/mbreissi/*` → `ghcr.io/edgecommons/*` (Dockerfiles, recipes).
   - GH Packages npm scope `@mbreissi` → `@edgecommons`; Maven `distributionManagement` URL → the
     org; git-dep URLs (Py/Rust) → `edgecommons/ggcommons`.
   - **Unchanged:** Java groupId `com.mbreissi`, PyPI name `greengrass-commons`, docs domain — all
     independent of the GitHub owner.
   - **Re-authorize the Cloudflare Workers Builds GitHub App** on `edgecommons` so docs keep
     auto-deploying after the transfer.
3. Create `edgecommons/.github` from `ecosystem/staging/org-dotgithub/` (profile README, health
   files, reusable workflows).

### Phase 2 — Stand up the registry ✅ (done 2026-06-29)
- Created `edgecommons/registry` (private) from `ecosystem/staging/registry/` (catalog + schema +
  validation workflow + CONTRIBUTING); `validate-registry` CI green.
- Shipped the CLI `list-components` command (reads the private catalog via `gh`); add `ggcommons
  add <name>` later.
- Add the docs-site "Components" page that renders the catalog.

### Phase 3 — Migrate the two adapters
For `gg-opcua-adapter` → `opcua-adapter` and `gg-modbus-adapter` → `modbus-adapter`:
- Create the org repo, push existing local history
  (`git remote add origin git@github.com:edgecommons/<name>.git && git push -u origin main`).
- Apply the standard layer: topics, adopt the reusable CI workflow, point the lib dependency at the
  published `ggcommons`, add README badges, open a `registry` PR.
- Re-validate on lab-5950x (GREENGRASS) + the permanent `ggcommons-modbus-sim` container + a KEP VM
  (OPC UA), per the monorepo validation matrix.

### Phase 4 — Make it self-perpetuating
- Update `create-component` + docs so a new component flows: scaffold → push to org → registry PR →
  CI inherited automatically.
- Optionally flag `templates/*` as GitHub **template repositories** (secondary click-to-use path).

## Billing & CI minutes

GitHub bills **each account separately**; a personal **Pro** subscription **cannot be converted**
into an org **Team** subscription — they are independent. Upgrading the org does not touch the
personal account, and vice versa.

Crucially, **Actions minutes do not pool across accounts** — workflows consume the *repo owner's*
allotment. Once repos move to `edgecommons`, the personal Pro 3,000 min/mo no longer applies to them;
only the org's plan (or a self-hosted runner / public visibility) does.

| Path | Private Actions min/mo | Notes |
|------|------------------------|-------|
| **Free org** | **2,000** | **chosen for now** — private, GitHub-hosted; manage with path filters/lean matrices/caching |
| Self-hosted runner (lab-5950x / WSL) | unlimited | deferred fallback if the 2,000 cap bites (RUNBOOK § Phase 0c) |
| Team org | 3,000 | $4 / user / mo, billed separately from personal Pro |
| Public repos | unlimited | considered then declined — keeping the ecosystem private for now |

## Migration checklist / risks

- [ ] Org created; CI-minutes path chosen.
- [ ] `mbreissi/ggcommons` transferred; local remote updated; old-URL redirect confirmed.
- [ ] ghcr namespace, GH Packages scope, Maven dist URL, Py/Rust git-deps repointed to `edgecommons`.
- [ ] Cloudflare Workers Builds GitHub App re-authorized on the org; docs auto-deploy verified.
- [ ] `edgecommons/.github` + `edgecommons/registry` created from staging; registry validation green.
- [ ] Adapters pushed, CI adopted, registry entries merged, re-validated on lab + sims.
- [ ] Decide personal Pro: keep (for remaining personal repos) or downgrade to Free.

## Staging in this branch

Branch `feat/ecosystem-edgecommons` carries everything that can be prepared before the org exists:
- `docs/ECOSYSTEM.md` — this plan.
- `cli/ggcommons_cli/commands/list_components.py` (+ test) — the registry-reading CLI command.
- `ecosystem/staging/` — content for the future `edgecommons/.github` and `edgecommons/registry`
  repos; see `ecosystem/staging/README.md` for the extraction steps.
- `ecosystem/RUNBOOK.md` — Phase 0–1 operational runbook (org + self-hosted runner, transfer,
  repoint) + `ecosystem/repoint-to-edgecommons.ps1`/`.sh` (dry-run-by-default coordinate repointing).
