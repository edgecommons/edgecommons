# Shared / Layered Configuration — design (PROPOSED, not built)

> Status: **design only.** Nothing here is implemented yet. This folds three deferred "shared"
> needs — shared config, shared credentials vault, shared parameter cache — into one effort,
> because they share one core problem. Java is the canonical reference; any build must land in all
> four libraries (Java/Python/Rust/TS) identically.

## 1. Motivation

In industrial deployments, components are not deployed in isolation: many run together on one
device / line and **must share identical configuration**. The canonical example is the tag set
`appId` / `site` / `shop` / `line` — every component on a line must carry the same values. Today
that config is **replicated per-component** in every recipe/config file. Goal: **define shared
values once**, and have components inherit them.

This is not just tags. The framework/infrastructure sections are the shared candidates, and in a
real deployment most of them would be shared:

- `tags`
- `logging` (the site-wide format; levels are often shared with per-component overrides)
- `heartbeat` (targets, measures, interval)
- `metricEmission` (target, namespace)
- the **source/provider** config for `parameters` and `credentials` — i.e. *where* params/secrets
  come from is shared; *which specific* params/secrets each component needs stays component-specific.

Component-specific = `component.global` / `component.instances` (business logic) and each
component's own param/secret selections.

## 2. The three needs are one problem

| Need | What it is | Reduces to |
|------|-----------|-----------|
| **Shared config** | A base config *document* every component deep-merges under its own. | A base-document **source** + a **merge** rule. |
| **Shared vault** | One encrypted *file* all components read, one owner writes. | A **device-wide file location** + the path/sync config carried by **shared config**. |
| **Shared parameter cache** | One encrypted cache *file*, offline-first, refreshed. | Same as the vault: a device-wide file location + config carried by shared config. |

Key insight: **shared config is the primary mechanism.** Once a base layer exists, the shared vault
and shared cache fall out of it — the base layer simply sets `credentials.vault.path` and
`parameters.cache.path` (plus their providers and the single-writer/sync-owner flags) to a common
**device-wide location**, so every component inherits the same paths and therefore the same physical
files. What remains genuinely separate is **(B) the device-wide filesystem location** for those two
files (a real OS/permissions problem), versus **(A) sourcing + merging the config document**.

So this design has two parts: **(A) layered config** (§4–§6) and **(B) the shared on-disk location**
for vault/cache (§7). (A) is the bulk; (B) is small but is the thing that was punted (every example
recipe uses the per-component `{ComponentFullName}` work dir — see `CREDENTIALS.md` status note).

## 3. Decisions already locked in (from review)

1. **Granularity: per-key override.** Not whole-section. Canonical case: device-wide default log
   level `INFO`, but bump one misbehaving component to `DEBUG` **while keeping the shared
   `logging.<lang>_format`**. Override `logging.level` only; inherit the format.
2. **Merge: field-level deep merge**, component layer wins over the base layer.
3. **Base layer is sourced via the same `-c` provider**, and must work across **all** sources
   (`FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT`) and **all modes** (GREENGRASS,
   STANDALONE, future K8S). Not a special case bolted onto one provider.
4. **Opt-in by default, explicit opt-out.** Layering is on by default (a component merges the
   shared base when one resolves); a component can explicitly opt out and use only its own config.
   *(Re-confirm the exact knob: a config key like `sharedConfig: false` and/or a `--no-shared-config`
   CLI flag.)*

## 4. The core question — where does the base layer live, per provider?

This is the crux (everything else hangs off it). For each `-c` source, the base layer is resolved
**the same way the component config is**, from a device/line/site-wide location:

| `-c` provider | Component layer (today) | **Base (shared) layer** |
|---------------|-------------------------|--------------------------|
| `FILE` | a JSON file path | a device-wide shared file: `$GGCOMMONS_SHARED_CONFIG` or a conventional path (e.g. `/etc/ggcommons/shared.json`); or an `extends` reference inside the component file. |
| `ENV` | JSON in `GGCOMMONS_CONFIG` | JSON in a separate `GGCOMMONS_SHARED_CONFIG` env var (in K8s, projected from a **shared ConfigMap** into every pod). |
| `GG_CONFIG` | this component's `ComponentConfig` (IPC `GetConfiguration` self) | the `ComponentConfig` of a **dedicated shared-config component** (e.g. `aws.proserve.greengrass.SharedConfig`), read via `GetConfiguration` for that name — the same cross-component IPC read that `CONFIG_COMPONENT` already uses. One shared-config component per line deployment. |
| `SHADOW` | this component's named shadow | a shared **device-level named shadow** (e.g. `ggcommons-shared`) every component reads. |
| `CONFIG_COMPONENT` | a config-management component serves this component's config | already the shared model in spirit — that component serves a shared base + per-component overlays (keyed by component name). |

**Per mode** mostly selects which provider is in play; it also fixes the device-wide *file* location
for §7. The unifying abstraction: a small **base-layer resolver per provider** that returns the base
document (or "none"); the merge engine (§5) is provider-agnostic and identical across libraries.

## 5. Merge semantics

- **Recursive field-level merge**, component-over-base. Objects merge key-by-key; **scalars and
  arrays are replaced** by the component value (arrays are not concatenated — a component that sets
  `heartbeat.targets` replaces the shared list; this keeps behavior predictable).
- Result: `effective = deepMerge(base, component)`.
- **Validation happens after merge**, against the canonical `schema/ggcommons-config-schema.json`.
  Individual layers are partial fragments (the base legitimately omits `component`; a component
  overlay may omit framework sections), so layers must **not** be validated as complete configs —
  only the merged result is. Top-level `additionalProperties:false` / `required:[component]` apply
  post-merge.
- **Hot reload:** a change to *either* layer re-runs the merge and fires the existing
  `ConfigurationChangeListener` path. (Base-layer watching uses each provider's existing
  change mechanism — file watch, shadow delta, GG config-update subscription.)

## 6. Opt-out

Default ON. A component opts out with a config key (proposed `sharedConfig: false`, read from the
*component* layer before merge) and/or a CLI flag (`--no-shared-config`). Opted-out → effective
config is the component layer alone.

## 7. The device-wide location for shared vault + cache (part B)

Independent of §4 (those are *files*, not config documents), but their **paths are set via the
shared config layer** so all components agree. The open problem is a single location that is
writable by the runtime user and addressable across modes:

- **GREENGRASS:** all components run as `ggc_user`, and a component's work dir is
  `ggc_user`-owned — so **the shared-config component's work dir is a viable shared location**:
  `/greengrass/v2/work/<SharedComponent>/vault` and `.../paramcache`, readable+writable by every
  component (same uid). (Verify cross-component work-dir perms on the target nucleus; if too tight,
  provision a dedicated `/var/lib/ggcommons` chowned to `ggc_user` in the shared component's
  `Install` lifecycle.) The single **sync owner** (already designed in `CREDENTIALS.md`) is
  naturally the shared-config / credentials-manager component.
- **STANDALONE / containers:** a shared host path or mounted volume (e.g. `/etc/ggcommons` or a
  named Docker volume) shared across the co-located containers.
- **K8S:** shared **ConfigMap** (shared config + non-secret values) and **Secret** (vault seed)
  projected into every pod; a shared **PVC** if the cache must persist across restarts.

The collision-avoidance `<thingName>/<componentName>/` **key namespacing is already shipped**
(see `CREDENTIALS.md`), so pointing components at one shared vault path is a config change, not a
rewrite. Same for the parameter cache.

## 8. Parity & non-goals

- The **merge algorithm** and the **per-provider base resolver** must be byte-for-byte equivalent
  across Java/Python/Rust/TS (add to the cross-language interop/conformance coverage).
- **v1 = two layers** (base ⊕ component). Multi-level (device < site < line < component) is a future
  extension via a composable/`extends`-able base — designed for, not built.
- Not a secrets-distribution mechanism: shared config carries the *source/provider* for secrets, not
  secret values (those still flow through the vault/central sync).

## 9. Open questions

1. Confirm the opt-out knob (config key vs CLI flag vs both) and re-confirm the default per §3.4.
2. `FILE`/`ENV`: settle on an explicit base reference (`extends`) **vs** a conventional path/env
   var **vs** both. Lean: support an explicit reference, default to a conventional device path.
3. `GG_CONFIG`: is the base a dedicated **shared-config component** (preferred — works with the
   existing per-component deploy model) or **nucleus-level** deployment config? Confirm the IPC read
   path matches `CONFIG_COMPONENT`'s.
4. Verify GREENGRASS cross-component work-dir readability on the target nucleus (decides §7 GG path).
5. Array semantics — confirm "replace, not concatenate" is acceptable for `heartbeat.targets` and
   any other shared list.
6. Precedence/source mixing — can the base come from a *different* provider than the component
   (e.g. component `FILE`, base `GG_CONFIG`)? Lean: no in v1 — base uses the same provider family.

## 10. Phasing (proposed)

1. **Merge engine + opt-out** (provider-agnostic, pure, identical in 4 libs) + post-merge validation
   + tests. No new sourcing yet (base passed in).
2. **Base resolvers per provider** (§4), starting with `FILE`/`ENV` (easiest, covers STANDALONE/K8S),
   then `GG_CONFIG` shared-config component, then `SHADOW`.
3. **Shared vault + cache location** (§7): point the example recipes' `credentials.vault.path` /
   `parameters.cache.path` at the device-wide shared location via the base layer; solve the GG
   `ggc_user` shared-dir + single-writer wiring; validate on the lab nucleus.
4. **Conformance**: cross-language merge test vectors; multi-component on-device validation.
