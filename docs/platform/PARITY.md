# Design — Four-language parity & implementation plan

> Companion to the other design docs. **Status: Phase 1 SHIPPED on `main` (v0.2.0), all four
> languages; Phase 2+ still proposed.** How the Kubernetes work lands across Java
> (canonical), Python, Rust, and TypeScript while preserving four-way parity, and the per-language
> specifics that shape it. Rule (CLAUDE.md + [[no-api-divergence-without-asking]]): define semantics in
> **Java first**, then mirror; any divergence is explicit and tracked.

---

## 1. Sequencing

Per change: **Java canonical → mirror to Rust/TS → mirror to Python**, with the schema synced as a
6-file commit (canonical + 5 copies, `schema/sync-schema.sh`; CI drift gate). Rust often *leads*
greenfield subsystems in this repo (streaming, parameters), so for new k8s pieces Rust may co-lead with
Java where the trait seam makes it cleaner — but Java remains the semantic reference.

Within the project plan: **Phase 0 (the behavior-preserving resolver refactor, DESIGN-core §9) lands in
all four before any Phase-1 Kubernetes feature.** Phase 0's oracle (existing suites green) is the safety
net; do not begin Phase-1 native facilities until Phase 0 is merged per language.

## 2. The work, by seam type

| Change | Java | Python | Rust | TS |
|---|---|---|---|---|
| `Platform`/`Transport` enums + `resolveProfile` + detector | new | new | new | new |
| Replace mode-switch with transport injection | `MessagingClient.java:42` | `MessagingClient.init` | `init_messaging` `lib.rs:461` (take `&Transport`) | `initMessaging` `edgecommons.ts:330` |
| Default-config-provider from profile | `EdgeCommons.java:378` | `edgecommons.py:139` | `cli.rs:158` | `cli.ts:72` |
| New `--platform`/`--transport` CLI; drop `-m` | `EdgeCommons.java:350-404` | `edgecommons.py:142-171` | `cli.rs:48-206` | `cli.ts:16-127` |
| `CONFIGMAP` config source | sealed: edit `permits` + builder | manager class | `ConfigSource` impl (open) | `ConfigSource` impl (open) |
| `prometheus` metric target | sealed: edit `permits` + switch; `client_java` | manager + `client_python` | feature `metrics-prometheus` + `prometheus-client` | `prom-client` |
| stdout-JSON logging sink | edit configurator (`JsonTemplateLayout`) | edit logging config | tracing JSON layer | edit logger |
| HTTP health endpoint + SIGTERM wiring | new (`addShutdownHook`) | new (`signal`) | new (`tokio::signal`) | exists pattern (`process.on`) |
| `env` KeyProvider (credentials) | new | new | new (feature `credentials`) | new |
| Optional `mountedDir`/`k8sSecret` `CentralVaultSource` | `Credentials.open` arm | mirror | feature `credentials` arm | mirror |
| PVC/StatefulSet streaming (deployment-only; engine unchanged) | docs + chart | docs + chart | docs + chart | docs + chart |
| Schema additions (`platform`/`transport`/`health`/`prometheus`/`identity`) | canonical | synced copy | synced copy | synced copy |

Sealed hierarchies (Java config providers + metric targets) cost an extra `permits`-clause edit vs the
open `trait`/`interface` seams in Rust/TS — a compile error if forgotten (safe), just more ceremony.

## 3. Per-language specifics

### Java (canonical)
- Builders are the construction path; **deprecated direct constructors coexist** — don't break the old
  surface when adding `platform()`/`transport()` setters. There is **no service-interface seam** and no
  `ServiceRegistry`; test against concrete services and reset process-global statics
  (`MessagingClient`, `MetricEmitter`) between tests.
- Sealed `ConfigProvider`/`MetricTarget`: new source/target edits the `permits` clause + the factory switch.
- Streaming uses Panama/FFM (`--enable-native-access=ALL-UNNAMED`); the image must keep that flag.
- **Java gap to close:** `StreamService.open` does **not** template-resolve the streaming config
  (`StreamService.java:46` — caller pre-resolves), while the Rust façade does. The k8s `buffer.path`
  templating (per-identity subdir) needs Java to resolve templates too, or document the caller contract.
- JaCoCo 90% gate must stay green through Phase 0 and Phase 1.

### Python (also see `libs/python/CLAUDE.md`)
- Builder/constructor only; **no DI/interface layer** (the old `edgecommons/di/` + `edgecommons/interfaces/`
  docs never shipped). pytest-style tests; don't add `unittest.TestCase`.
- AWS deps lazy-imported (boto3) so optional features don't break core; the `awsSsm`/Secrets-Manager paths
  gate at runtime (not compile features).
- **Carries the largest pre-existing parity debt** (see [[edgecommons-python-parity-backlog]] — init-order
  C1/C2, dead file-logging, H/M bugs). The Phase-0 resolver refactor touches the exact init path that
  backlog flags; **fold the k8s init-order changes into that remediation** rather than layering on top of
  known-buggy ordering. This is the riskiest per-language surface.

### Rust
- Builder-only greenfield; has the **service-interface seam** (`MessagingService`/`MetricService` traits +
  `Arc<dyn …>`), so transport injection and new targets are localized and testable.
- **Cargo features** are the gating idiom — add `metrics-prometheus` (off by default) for the prometheus
  target, mirroring `cloudwatch`/`streaming`/`credentials-*`/`parameters-aws`. The `greengrass` feature is
  **Linux/WSL-only** ([[rust-greengrass-build-wsl]]); the resolver must fail fast when `platform=GREENGRASS`
  on a `greengrass`-feature-less build instead of today's silent `Ok(None)` messaging (`lib.rs:499-502`).
- Uses `Zeroizing` for DEKs (credentials) — keep when adding the `env` KeyProvider.
- **Rust gap:** tracing layers can't be reconfigured after install (`logging.rs:24-29`) — the stdout-JSON
  sink must be selected at install time; document the hot-reload limitation vs Java's full rebuild.
- Keep CI matrices Linux-only (recent ci changes); a `--platform greengrass` path still only builds/tests
  on Linux/WSL.

### TypeScript
- Builder-only greenfield; **service-interface seam** (`IMessagingService`/`MetricService`) — clean
  transport injection.
- **SIGTERM handling already exists** (`process.on('SIGTERM'|'SIGINT')` in `edge_verify.ts`/
  `config_provider.ts`) — the canonical pattern the other three mirror for FR-HB-2.
- No official Prometheus client → use `prom-client` (community, ships TS types); a **documented, accepted
  divergence** (same situation as other Node ecosystem libs).
- **`receiveOwnMessages` defaults `false`** here vs `true` in Java/Python/Rust (`edgecommons.ts:171`) —
  **resolved**: converge to the Java-canonical `true` in Phase 0 (DESIGN-core §12 #2).

## 4. Pre-existing parity gaps this effort touches (track, don't silently fix)

| Gap | Where | Handling |
|---|---|---|
| `receiveOwnMessages` default (TS `false` vs others `true`); Rust treats `false` as no-op | messaging | **resolved**: converge to Java-canonical `true` + fix Rust no-op, in Phase 0 (DESIGN-core §12 #2) |
| Java streaming config not template-resolved (Rust is) | streaming | close as part of k8s `buffer.path` templating |
| Logging: facade/`globalControl`/isolated configurator are **Java-only**; stdout-JSON sink greenfield in all four | logging | add sink in lockstep; no shared sink seam → highest drift risk |
| Rust tracing can't reconfigure after install | logging | document; sink fixed at install |
| Metrics: Java/Python silently skip messaging injection (NPE risk); Rust/TS hard-error | metrics | align toward fail-fast where feasible |
| Python init-order + dead file-logging + H/M backlog | core/init | fold k8s init changes into the Python remediation |
| Credentials: `env` KeyProvider documented but unbuilt | credentials | build `env` KeyProvider in all four — **committed** (FR-CRED-3 / FR-CRED-6: the offline-capable k8s default) |
| Credentials: mounted-secret `CentralVaultSource` (ESO/CSI shape) | credentials | **optional** (FR-CRED-4 MAY); add if/when delegation to ESO/CSI is adopted |

## 5. Testing & verification parity

- **Phase-0 oracle:** every pre-existing suite passes in all four after the resolver refactor (tests
  re-pointed from `-m` to `--platform/--transport`); interop 32/32; vault vectors unchanged;
  `sync-schema.sh --check` green (DESIGN-core §9, REQUIREMENTS NFR-COMPAT).
- **Phase-1 new tests (each lang):** resolver precedence + invalid-combo guard; auto-detect signal matrix;
  ConfigMap directory-watch re-arm after `..data` swap (the highest-risk parity item — test in all four);
  prometheus `/metrics` scrape + lifecycle-no-op; SIGTERM→unsubscribe; streaming reschedule-preserves-backlog
  on PVC; **disconnect-tolerance fault injection** (NFR-DISCONNECT-1) across credentials/parameters/streaming/
  messaging.
- **Cross-language:** the interop harness node entrypoints (`test-infra/interop/*_node/*`) migrate to the
  new flags simultaneously so the matrix stays green.

## 6. Docs & schema obligations

Update each subsystem's `doc/`/`docs/` page and the cross-language design docs, plus the CLAUDE.md
"Standard CLI contract" section (the `-m` contract changes). New `docs/platform/` (this set) documents the
platform model and deployment. Every schema edit is the 6-file synced commit. `docs/SHARED_CONFIG.md`
(layered config) is the natural place to also document platform/transport layering.
