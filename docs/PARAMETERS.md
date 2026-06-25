# Parameters — design (pluggable parameter service)

**Status: Phase 1 (Core) SHIPPED in all 4 languages (Rust reference, then Python/Java/TS).**
Pluggable `ParameterSource` (`env` + `mountedDir` + `awsSsm`), source-aware offline-first cache
(persistent-encrypted via the credentials `LocalVault` for remote sources, in-memory for local),
selective bootstrap + background refresh, typed accessors, and the `gg.parameters()` accessor are
all wired and tested. Open questions §9 were resolved as the recommended defaults (own
`cache.keyProvider`; encrypt the persistent cache uniformly; `gg.parameters()`; ship all three
sources in phase 1; `securePaths` list for `mountedDir` secret marking). Phase 2 (audit+metrics
bridge) and Phase 3 (write-back, K8s-API watch, more cloud sources) remain.

A new, **independent** ggcommons subsystem — a peer of `config` / `messaging` / `metrics` /
`credentials` — giving components offline-capable, **source-agnostic** access to externalized
parameters/config. Exposed as `gg.parameters()` → `IParameterService`.

The **source is pluggable** (`ParameterSource`): AWS SSM Parameter Store is the default cloud
source, and Kubernetes / Docker / env / file sources cover HOST and KUBERNETES deployments. The
service (cache, refresh, typed reads, offline-first) is identical regardless of source.

## 1. Why a separate, source-pluggable service

Parameter Store and Secrets Manager are different services for different jobs, and components
routinely use **both at the same time**:

- **Secrets Manager** (→ `gg.credentials()`): purpose-built secrets — rotation, always-KMS,
  vault-grade handling (encrypted-at-rest, MAC, monotonic versions, audit).
- **Parameters** (→ `gg.parameters()`): predominantly **configuration** — endpoints, feature
  flags, tuning — `String`/`StringList` with `SecureString`/secret values as one option. Its
  signature operation is **fetch-a-subtree** (`GetParametersByPath`), meaningless in a vault.

Folding SSM into `credentials` as a `CentralVaultSource` was rejected (wrong abstraction —
forces config-grade params through the secrets-vault model, conflates two concerns). And the
source must be **pluggable**, because ggcommons runs across multiple platforms: `GREENGRASS`
(AWS-native → SSM via TES) and **`HOST` / `KUBERNETES` for Docker/bare containers/k8s**, where the
equivalent is K8s ConfigMaps/Secrets, Docker secrets, or env — not SSM.

## 2. Pluggable parameter sources

```
ParameterSource                                  # the seam — one method pair
  fetch(name) -> Optional<ParamValue>
  fetch_by_path(path, recursive) -> Map<String, ParamValue>

ParamValue { value: bytes/str, secure: bool, version: Option<String> }
```

Built-in implementations (selected by `parameters.source.type`):

| Source | `type` | Backend | Typical platform |
|--------|--------|---------|--------------|
| **AWS SSM Parameter Store** | `awsSsm` | `GetParameter(s)` / `GetParametersByPath`, `WithDecryption=true` | GREENGRASS (TES) / EC2 / any AWS |
| **Mounted directory** | `mountedDir` | a directory tree of files — **K8s ConfigMap/Secret volume mounts**, **Docker secrets (`/run/secrets`)**, bare config dirs. Subdirs → paths, files → params; configured subpaths flagged `secure`. | HOST / KUBERNETES / Docker |
| **Environment** | `env` | env vars under a prefix (`MYAPP_*` → `/myapp/*`) | HOST / containers |
| *custom* | `custom` | host-supplied `ParameterSource` instance (HashiCorp, Azure App Config, GCP, K8s API client, …) | any |

The `mountedDir` source is the idiomatic K8s/Docker mechanism (config/secrets projected as
files) and needs **no API client or RBAC** — the orchestrator already did the fetch + mount.
A K8s-API-client source (live ConfigMap/Secret reads + watch) is a possible later `custom`
impl, but mounts cover the common case. AWS SSM, Azure, HashiCorp, etc. are the remote sources
that make the offline cache (§3) matter.

## 3. Offline-first cache (source-agnostic; persistence is per-source)

`ParameterService` sits **above** the source and is offline-first: reads serve from a cache and
never block; a failed refresh keeps serving last-known values.

- **Remote sources** (SSM, Azure, HashiCorp): a **persistent cache** is essential for offline
  resilience. Because it can hold secret values, the on-disk cache is **encrypted at rest**,
  reusing the credentials crypto (AES-256-GCM/HKDF/HMAC) + a `KeyProvider` (file/KMS/PKCS#11).
- **Already-local sources** (`mountedDir`, `env`): the backend *is* local and always available,
  so persistence is redundant (and re-writing a K8s in-memory Secret to disk would be a
  regression). These default to an **in-memory cache** (read-through); no encrypted disk copy.
- So cache persistence is a **per-source default** (`persist: true` for remote, `false` for
  local), overridable. The encrypted-disk cache is the same store machinery the vault uses
  (atomic temp→rename under an advisory lock, reload-on-change, per-param upstream version for
  change detection). Fully-offline disk caching needs a fully-offline KEK custodian
  (`file`/`pkcs11`); `kms` needs connectivity at startup only (same trade as the vault).

This is a **read-through mirror**, not writable (no `put` upstream in v1 — matches credentials'
pull-only model). `refresh()` re-pulls from the source.

## 4. Service surface

```
IParameterService
  get(name) -> Optional<String>                  # plain or decrypted secure value
  get_by_path(path, recursive=true) -> Map<String,String>
  get_int(name) / get_bool(name) / get_json(name)
  get_string_list(name) -> List<String>
  names(prefix="") -> List<String>               # cached names (metadata, no values)
  refresh() -> ()
  stats() -> { parameter_count, last_refresh_age_ms, refresh_failures, source }
```

- Reads always come from the cache (offline-first); `get*` never makes a network call.
- `{ThingName}`/`{ComponentName}` templating on configured names/paths (per-device trees).
- **Secure values** (SSM SecureString, a `mountedDir` secret subpath, …) are never logged,
  redacted in audit/diagnostics, zeroized where the language allows.

## 5. Refresh / sync model (mirrors the credentials SyncEngine)

- **Selective**: the component declares the `names`/`paths` it needs (least privilege + bounded
  cost). **Bootstrap** on start + **periodic background refresh** + on-demand `refresh()`.
- Offline-tolerant: a failed pull is logged + counted; cached values retained. (Local sources
  effectively never fail to refresh.)

## 6. Config schema (`parameters` section)

```yaml
parameters:
  source:
    type: "awsSsm"               # awsSsm | mountedDir | env | custom
    # --- awsSsm ---
    region: "us-east-1"
    endpointUrl: "..."           # optional (floci/LocalStack/VPC endpoint)
    withDecryption: true
    # --- mountedDir ---
    # root: "/etc/config"        # e.g. a K8s ConfigMap mount
    # securePaths: [ "/etc/secrets" ]   # subpaths whose files are secret values
    # --- env ---
    # prefix: "MYAPP_"
  cache:
    persist: true                # default: true for remote sources, false for local
    path: "/greengrass/v2/work/{ComponentFullName}/param-cache"
    keyProvider: { type: "file" } # file | kms | greengrass | pkcs11 (same shapes as credentials)
  refreshIntervalSecs: 300
  bootstrapOnStart: true
  sync:
    names: [ "/myapp/timeout" ]
    paths: [ { path: "/myapp/", recursive: true } ]
```

`gg.parameters()` returns `None`/`null` when there is no `parameters` section (like
`gg.credentials()` / `gg.streams()`). A `custom` source is supplied programmatically (a builder
hook), not via config.

## 7. Reuse vs. new code

Reused (shared internal primitives, no duplication): `crypto` (AES-256-GCM/HKDF/HMAC),
`keyprovider` (file/KMS/PKCS#11), the atomic-write + advisory-lock + reload-on-change store, and
the background-refresh engine pattern. (Implementation note: extract those credentials internals
into a small shared module that both `credentials` and `parameters` depend on, rather than
`parameters` reaching into `credentials`.)

New: the `ParameterSource` trait + `AwsSsmSource` / `MountedDirSource` / `EnvSource`, the cache
store (persistent-encrypted + in-memory variants), `ParameterService` + typed accessors, the
`parameters` config parser, and `gg.parameters()` wiring.

## 8. Cross-language parity & phasing

Rust reference first, then Python/Java/TS to parity; unit tests per source + a floci-gated SSM
integration test (floci has `ssm`). The encrypted disk-cache format is normative across
languages (cross-language test vectors, like the vault).

1. **Core**: `ParameterService` + the `ParameterSource` seam + `AwsSsmSource` + `mountedDir` +
   `env` sources + offline cache (persistent-encrypted & in-memory) + selective bootstrap/refresh
   + `get`/`get_by_path`/typed + config + `gg.parameters()` wiring (4 langs).
2. **Audit + metrics**: reuse the credentials `AuditSink` (param access; secure values redacted)
   + a `parameters` metric (count, last-refresh age, failures).
3. (later) write-back, K8s-API source + watch, more cloud sources.

## 9. Open questions (for review)

1. **KEK ownership** — proposed: parameters has its own `cache.keyProvider` (independent, can
   point at the same custodian as credentials). Agree, or share one device KEK?
2. **Encrypt whole persistent cache vs. only secure values** — proposed: encrypt uniformly
   (simpler; SecureStrings force it; loses plaintext debuggability). OK?
3. **Naming** — `gg.parameters()` / `IParameterService` (vs `gg.config2()` / `gg.ssm()`).
4. **Source set for phase 1** — `awsSsm` + `mountedDir` + `env` in the first cut (recommended,
   so K8s/Docker work out of the box), or `awsSsm` first and the local sources as a fast-follow?
5. **`mountedDir` secret marking** — config `securePaths` list (proposed) vs. a convention
   (e.g. anything under a `*/secrets/` path) vs. treat all mounted files as plain.
