# Parameters (AWS SSM Parameter Store) — design

**Status: DESIGN (for review). Not yet implemented.**

A new, **independent** ggcommons subsystem — a peer of `config` / `messaging` / `metrics` /
`credentials` — that gives components offline-capable access to **AWS SSM Parameter Store**.
Exposed as `gg.parameters()` → `IParameterService`.

## 1. Why a separate service (not a credentials backend)

Parameter Store and Secrets Manager are different services for different jobs, and components
routinely use **both at the same time**:

- **Secrets Manager** (→ `gg.credentials()`): purpose-built secrets — rotation, always-KMS,
  replication. Vault-grade handling (encrypted-at-rest, MAC, monotonic versions, audit).
- **Parameter Store** (→ `gg.parameters()`): predominantly **configuration/parameters** —
  mostly plain `String` / `StringList` (endpoints, feature flags, tuning), with `SecureString`
  as one option. Its signature operation is **`GetParametersByPath`** (fetch a whole config
  subtree) — which has no meaning in a secrets vault.

Folding SSM into `credentials` as a `CentralVaultSource` was rejected: it forces config-grade
parameters through the secrets-vault model (wrong abstraction) and conflates two orthogonal
concerns. So `parameters` is its own subsystem. Secrets continue to flow through `credentials`.

## 2. Offline-first with a persistent encrypted cache (core requirement)

Like the credentials vault, the parameters service is **offline-first**: SSM is the source of
truth, but **reads are served from a persistent local cache** and never block on the network. If
a refresh fails (offline), the last-known values keep serving.

Because the cache can hold `SecureString` values (sensitive), the **on-disk cache is encrypted
at rest** — reusing the credentials crypto primitives (AES-256-GCM + HKDF/HMAC) and a
`KeyProvider` (file / KMS / PKCS#11). The whole cache is encrypted uniformly (SecureStrings
require it; plain Strings cost ~nothing extra and it avoids per-value classification logic).

- **KEK custodian:** the parameter cache has its **own** `keyProvider` config (mirrors
  `credentials.vault.keyProvider`), so the two subsystems stay independent; it can point at the
  same keyfile/KMS key/HSM as credentials if an operator wants one device KEK.
- **Fully-offline custodians:** `file` (default) and `pkcs11` (local HSM) allow reads with **no
  network ever**. `kms` requires connectivity *at startup* to unwrap the DEK (then in-memory),
  same trade as the credentials vault — so `file`/`pkcs11` are recommended when hard-offline
  parameter reads after a cold restart are required.
- Separate store file from the secrets vault (different lifecycle, refresh cadence, source).
  Atomic temp→rename writes under a cross-process advisory lock; reload-on-change (the same
  machinery the vault uses). The cache records each parameter's **SSM version** for change
  detection (refresh only re-pulls when the upstream version changed).

This is a **read-through cache/mirror**, not a writable store: there is no local `put` to SSM in
v1 (matching the credentials pull-only model). `refresh()` re-pulls from SSM.

## 3. Service surface

```
IParameterService
  get(name) -> Optional<String>                  # String or decrypted SecureString
  get_by_path(path, recursive=true) -> Map<String,String>
  get_int(name) / get_bool(name) / get_json(name)
  get_string_list(name) -> List<String>          # StringList -> list
  names(prefix="") -> List<String>               # cached parameter names (metadata, no values)
  refresh() -> ()                                 # force an immediate pull from SSM
  stats() -> { parameter_count, last_refresh_age_ms, refresh_failures }
```

- `{ThingName}` / `{ComponentName}` templating is applied to configured paths/names (per-device
  parameter trees) before fetch.
- All reads come from the local cache (offline-first); `get*` never makes a network call.
- `SecureString` values are **sensitive**: never logged, redacted in any audit/diagnostic output
  (and zeroized where the language supports it). A type/flag distinguishes secure vs plain so
  callers/audit can treat them accordingly.

## 4. Refresh / sync model (mirrors the credentials SyncEngine)

- **Selective**: the component declares the `names` and/or `paths` it needs (least privilege +
  bounded SSM cost/rate). Nothing else is fetched.
- **Bootstrap** on start (configurable) + **periodic background refresh** at
  `refreshIntervalSecs` + on-demand `refresh()`.
- `GetParametersByPath` (paginated, `WithDecryption=true`, `Recursive`) for paths;
  `GetParameters` for explicit name lists.
- Refresh is **offline-tolerant**: a failed pull is logged + counted (`refresh_failures`),
  cached values are retained. First-ever start with no cache + offline → empty (documented).

## 5. Modes

Parameter Store is a direct AWS API (no Greengrass IPC), so the service works identically in
both runtime modes via the AWS SDK's default credential chain:
- **GREENGRASS**: TES (the component's role) — requires `ssm:GetParameter(s)` /
  `ssm:GetParametersByPath` (+ `kms:Decrypt` for SecureStrings) on the TES role.
- **STANDALONE**: ambient AWS credentials (env / profile / instance role).

The AWS SDK (`ssm`) is an **optional, feature-gated** dependency (`parameters-aws` /
optionalDependency / optional Maven artifact) so components that don't use SSM don't carry it —
exactly like the Secrets Manager dep.

## 6. Config schema (`parameters` section)

```yaml
parameters:
  region: "us-east-1"            # optional; default chain otherwise
  endpointUrl: "..."             # optional (floci/LocalStack/VPC endpoint)
  cache:
    path: "/greengrass/v2/work/{ComponentFullName}/param-cache"
    keyProvider: { type: "file" } # file | kms | greengrass | pkcs11 (same shapes as credentials)
  refreshIntervalSecs: 300
  bootstrapOnStart: true
  withDecryption: true           # default true; SecureStrings resolved
  sync:
    names: [ "/myapp/timeout", "/myapp/feature-flags" ]
    paths: [ { path: "/myapp/", recursive: true } ]
```

`gg.parameters()` returns `None`/`null` when there is no `parameters` section (like
`gg.credentials()` / `gg.streams()`).

## 7. Reuse vs. new code

Reused from the credentials subsystem (shared primitives, no duplication):
- `crypto` (AES-256-GCM, HKDF, HMAC), `keyprovider` (File/KMS/PKCS#11), the atomic-write +
  advisory-lock + reload-on-change store mechanics, and the background-refresh engine pattern.

New code (the `parameters` module, per lang):
- `AwsSsmSource` (SDK calls: GetParameter(s), GetParametersByPath), the encrypted **parameter
  cache store**, the `ParameterService` + typed accessors, the `parameters` config parser, and
  the `gg.parameters()` runtime wiring.

(Implementation note: extract the shared store/crypto bits into a small internal module so both
`credentials` and `parameters` depend on it, rather than `parameters` reaching into
`credentials` internals.)

## 8. Cross-language parity & phasing

Same approach as credentials: **Rust reference first**, then Python/Java/TS ports to parity,
each with unit tests + a floci-gated integration test (floci has `ssm` — verify
GetParameter/GetParametersByPath + SecureString decryption). The on-disk cache format is
normative across languages (a cache written by one lib is readable by another on the device),
covered by cross-language test vectors like the vault.

Phasing:
1. **Core**: `ParameterService` + encrypted persistent cache + `AwsSsmSource` + selective
   bootstrap/refresh + `get`/`get_by_path`/typed + config + `gg.parameters()` wiring (4 langs).
2. **Audit + metrics**: reuse the credentials `AuditSink` (param access events; SecureString
   values redacted) and a `parameters` metric (count, last-refresh age, failures) via the
   metrics bridge.
3. (Optional, later) write-back (`put` → SSM), change-notification, more backends.

## 9. Open questions (for review)

1. **Own KEK vs shared with credentials** — proposed: own `parameters.cache.keyProvider`
   (independent), able to point at the same custodian. Agree, or share one device KEK?
2. **Encrypt the whole cache vs. only SecureStrings** — proposed: encrypt uniformly (simpler;
   loses plaintext debuggability of the cache file). Acceptable?
3. **Service name** — `gg.parameters()` / `IParameterService` (vs `gg.ssm()` / `ParameterStore`).
4. **Plain-config convenience** — should non-secret params optionally feed the `config`
   subsystem (e.g. `config.secretRef`-style `{"$param": "..."}` indirection), or stay strictly
   behind `gg.parameters()`? (Lean: keep separate for v1.)
