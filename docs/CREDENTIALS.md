# Credentials & Local Vault — design

A generic, cross-cutting **secrets** subsystem for ggcommons: a secure, encrypted-at-rest
**local vault** on the device that can run **standalone** or be **initialized and kept
up to date from a central cloud vault** (AWS Secrets Manager first; pluggable). It is a
peer subsystem to `config`, `messaging`, `metrics`, and `heartbeat` — usable by any
component, not specific to streaming. Streaming's Kafka/Kinesis credential needs become
one consumer of it (this is what fixes `TELEMETRY_STREAMING.md` §7).

---

## 1. Decisions (settled)

1. **Generic secrets, not credential-typed storage.** The vault stores **named, versioned,
   opaque byte blobs** with metadata. Typed convenience views (AWS creds, basic-auth,
   TLS bundle, Kafka SASL) are thin accessors *over* opaque secrets — the store never
   parses a secret's contents (mirrors streaming: payloads are opaque bytes).
2. **Offline-first.** The local vault is the authoritative **read** path. The central vault
   is the upstream source of truth that *seeds and refreshes* it. A component keeps working
   from cached secrets when the cloud is unreachable — essential at the edge.
3. **Pull-oriented in v1.** Central → local sync only. Local `put` writes **local-only**
   secrets (standalone use), never pushed upstream. Bidirectional/push is a later phase.
4. **Per-language implementations against ONE normative spec — NOT a shared native core.**
   This is the deliberate inverse of the streaming decision, and the rationale differs:
   streaming chose a single Rust core for **throughput + identical durable-log byte format**;
   credentials have **no perf driver** (tiny data, low write rate) and adding a native
   library to *every* component that wants a secret is too heavy for a broadly-adopted
   primitive. Each lib uses its mature, audited crypto library (Java JCE/AES-GCM, Python
   `cryptography`, Node `crypto`, Rust `aes-gcm`/`aws-lc-rs`) against a **normative on-disk
   vault format** (§4) plus a **shared test-vector suite** (§4.4) so a vault written by one
   language is byte-compatible and decryptable by another on the same device. (Alternative
   rejected in §11.)
5. **Envelope encryption with a pluggable key custodian.** Secrets are sealed with a per-vault
   Data Encryption Key (DEK); the DEK is wrapped by a Key Encryption Key (KEK) held by a
   pluggable `KeyProvider`. The abstraction deliberately unifies **remote custodians** (AWS
   KMS) and **local hardware custodians** (HSM / TPM 2.0 / secure element via PKCS#11) — both
   *unwrap the DEK without ever exposing the KEK* — alongside software fallbacks (keyfile /
   env). The KEK never lands on disk in plaintext. **On Greengrass the default is KMS-via-TES
   with a local keyfile fallback; where the device exposes an HSM/TPM, that is preferred** —
   it gives hardware-protected keys *and* fully-offline unlock (no cloud round-trip), which a
   cloud KMS cannot. (Confirmed direction.)
6. **Depend on the interface.** Components use `ICredentialService` (the testable seam),
   obtained as `gg.credentials()` — same pattern as `gg.messaging()` / `gg.streams()`.

## 2. Non-goals (v1)

- Not a general KV/feature-flag store — secrets only (small, sensitive, versioned).
- Not an HSM/TPM-backed keystore in v1 (OS-keyring/TPM `KeyProvider` is a later add).
- No interactive passphrase prompts (components are non-interactive).
- No central **write** (push) — central is read-upstream only in v1.
- Not a replacement for IAM/least-privilege at the cloud edge — it *complements* it
  (selective sync + central resource policy bound what a device can read).

## 3. Architecture

```
component code ──> ICredentialService (gg.credentials())
                        │
                        ├── LocalVault  (encrypted-at-rest store; the read path)
                        │     ├── AEAD records (per secret/version)         §4
                        │     ├── DEK (wrapped) ── KeyProvider (KEK)        §5
                        │     └── change notifications (rotation hot-reload)
                        │
                        └── SyncEngine  (background; offline-first)         §6
                              └── CentralVaultSource  (pluggable)
                                    ├── AwsSecretsManagerSource  (primary)
                                    ├── AwsSsmParameterStoreSource
                                    └── (HashiCorp / Azure KV — later)
```

- **Reads** are served entirely from the LocalVault (decrypt in memory; never block on cloud).
- **SyncEngine** runs in the background: bootstrap at startup, periodic refresh, on-demand
  `refresh()`, and rotation handling. It writes new versions into the LocalVault and fires
  change listeners.
- One vault instance per component by default (isolation; lives in the component work dir).
  A **shared device vault** (fixed path + shared KEK) is opt-in for cross-component secrets.

## 4. The local vault (normative — identical across all bindings)

### 4.1 On-disk format

A single small file, written **atomically** (write temp → fsync → rename), JSON for
debuggability (CBOR is an allowed compact alternative under the same schema):

```jsonc
{
  "format": 1,
  "vaultId": "<uuid>",                      // stable id; part of rollback protection
  "kek": {                                  // how to obtain/unwrap the DEK
    "provider": "file" | "kms" | "env" | "greengrass",
    "wrappedDek": "<base64>",               // DEK encrypted by the KEK (KMS ciphertext, or AES-KW)
    "kmsKeyId": "arn:aws:kms:...",          // provider=kms
    "alg": "AES-256-GCM"
  },
  "secrets": {
    "prod/db/password": {
      "versions": [
        {
          "version": "00000003",            // monotonic, zero-padded; newest last
          "createdMs": 1781990000000,
          "labels": { "AWSCURRENT": true },
          "ttlSecs": 3600,
          "source": "central" | "local",
          "centralVersionId": "…",          // SM VersionId, for change detection
          "nonce": "<base64 96-bit>",
          "ciphertext": "<base64>",         // AES-256-GCM(plaintext); tag appended
          "contentType": "application/octet-stream"
        }
      ]
    }
  },
  "mac": "<base64>"                          // HMAC-SHA256 over canonical(secrets+meta) under a MAC key derived from the DEK
}
```

- **AEAD**: AES-256-GCM, 96-bit random nonce, 128-bit tag. **AAD = `format ‖ vaultId ‖ name ‖ version`**
  binds each ciphertext to its identity and version → prevents copy/swap of records and
  in-file rollback.
- **Vault MAC**: an HMAC over the whole record set (under a key HKDF-derived from the DEK,
  separate from the encryption key) detects truncation/tampering of the file structure and,
  with the monotonic version counters, **rollback** of the whole file.
- **Versioning**: keep the newest *N* versions per secret (configurable, default 2) so a
  consumer mid-rotation can still read the previous value during a grace window.

### 4.2 Read/write operations

Atomic, crash-safe, and concurrency-safe (advisory file lock for multi-process; in-process
RWLock). Writes are rare and small, so a full-file rewrite-and-rename per change is fine
(no segment store needed, unlike streaming).

### 4.3 In-memory hygiene

- Plaintext secrets and the DEK are held in **zeroizing** buffers, wiped on drop.
- Secret values are **never logged** and never serialized into config snapshots or metrics.
- Optional `mlock` of the DEK page to keep it out of swap (best-effort, platform-gated).
- Decrypt **on access**, cache decrypted value for a short configurable window, then drop.

### 4.4 Cross-language byte-compatibility (test vectors)

Because four languages each implement the format, the repo ships a **`vault-test-vectors/`**
suite: fixed DEK + plaintext + nonce + AAD → expected ciphertext/tag, plus a fully-formed
sample vault file. Every binding has a conformance test that (a) decrypts the canonical
vault and (b) re-encrypts a known input to the exact expected bytes. CI fails on any drift.
This is how interoperable encrypted formats (age, JWE) stay consistent without a shared core.

## 5. Key providers / custodians (where the KEK lives)

`KeyProvider` is the unlock seam: given the on-disk `wrappedDek`, it returns the unwrapped
DEK **without exposing the KEK**. The DEK lives only in memory (zeroizing) after unlock. The
same interface covers software keys, a remote KMS, and on-device hardware — the vault format
is identical across all of them (only the `kek.provider` field differs).

| Provider | KEK custodian | Unwrap | Offline unlock | Best for |
|----------|--------------|--------|:--------------:|----------|
| **hsm / tpm** | On-device **HSM / TPM 2.0 / secure element** (PKCS#11; KEK non-extractable) | hardware unwrap via PKCS#11 | ✅ | **hardened edge devices** — hardware-protected *and* offline |
| **kms** | AWS KMS CMK (KEK never leaves KMS) | `kms:Decrypt` of `wrappedDek` via AWS creds/TES | ❌ (needs cloud at unlock) | cloud-connected; CMK key policy is a real access gate |
| **greengrass** | KMS through **TES** + device role | `kms:Decrypt` with the device role | ❌ | GG default (zero extra config) — falls back to **file** when cloud is unreachable |
| **file** | 32 random bytes in a `0600` key file | local AES-Key-Wrap | ✅ | standalone / bare container; offline fallback |
| **env** | KEK (base64) from an env var | local AES-Key-Wrap | ✅ | dev / k8s secret-as-env |

**Default selection (confirmed):**
- GREENGRASS: **HSM/TPM if the device exposes one**, else **KMS-via-TES with a `file` keyfile
  fallback** (so a cold boot with no cloud can still unlock).
- STANDALONE: **hsm/tpm if present**, else **file**.

The HSM/TPM provider is treated as a first-class custodian from the start (not a "later"
add-on): the `KeyProvider` interface and the on-disk envelope are designed so a PKCS#11
backend slots in without a format change. Hardware keys give the property a cloud KMS can't —
**hardware-grade protection with fully-offline unlock** — which is the right posture for an
unattended edge device. (Phase 1 ships `file`; `hsm/tpm` + `kms` land in phase 2, §12.)

## 6. Central vault & sync

### 6.1 `CentralVaultSource` (pluggable)

```
fetch(name) -> { bytes, centralVersionId, labels, createdMs }
fetchMany(names | prefix | tagFilter) -> [...]
list(prefix | tagFilter) -> [SecretMeta]
```

- **AwsSecretsManagerSource** (primary): `GetSecretValue` / `BatchGetSecretValue` /
  `ListSecrets`; maps SM `VersionId` + staging labels (`AWSCURRENT`/`AWSPREVIOUS`) to vault
  versions. Auth = AWS default chain → **TES on Greengrass** (no extra code), standard chain
  standalone.
- **AwsSsmParameterStoreSource**: `SecureString` parameters by path (cheaper alternative).
- **HashiCorpVaultSource / AzureKeyVaultSource**: later, same trait.
- **Host-SDK callback option**: a binding may supply its own fetch callback so a component
  reuses the host's AWS SDK / credentials config instead of the built-in source.

### 6.2 Sync engine behavior

- **Bootstrap (init)**: at startup, pull the configured secret set (explicit names, prefixes,
  or tag filter) into the local vault. First run with no cloud → vault stays empty/local-only.
- **Refresh (update)**: every `refreshIntervalSecs`, and on-demand via `refresh(name?)`. Uses
  `centralVersionId` to pull only **changed** secrets. Updates local, fires change listeners.
- **Selective sync**: a component declares only the secrets/prefixes it needs → least
  privilege + small blast radius + smaller vault.
- **Rotation**: a new central version is pulled as a new vault version; the previous version
  is retained for `rotationGraceSecs` so in-flight consumers don't break; listeners notified
  so consumers (e.g. a Kafka producer) can re-auth.
- **Resilience**: central unreachable → keep serving cache, emit a `sync-staleness-age` metric;
  never fail a read because the cloud is down.
- **TTL**: a secret past its `ttlSecs` triggers a forced refresh on next access (or background).

## 7. Public API (cross-language parity)

`gg.credentials()` → `ICredentialService`:

```
// generic, opaque
get(name) -> Secret                  // latest version (decrypted, zeroizing)
getVersion(name, version) -> Secret
getBytes(name) -> bytes
getString(name) -> string            // utf-8
getJson(name) -> object
exists(name) -> bool
list(prefix?) -> [SecretMeta]        // metadata only, never values
put(name, bytes, {labels?, ttl?})    // local-only secret (standalone)
rotateLocal(name, bytes)             // new local version
delete(name)
refresh(name?) -> awaitable          // pull from central now
addChangeListener(name, listener)    // rotation/refresh hot-reload (mirrors ConfigurationChangeListener)
removeChangeListener(...)

// typed convenience views (thin adapters; no new storage)
getAwsCredentials(name) -> {accessKeyId, secretAccessKey, sessionToken?, expiry?}
getBasicAuth(name)      -> {username, password}
getTlsBundle(name)      -> {certPem, keyPem, caPem?}
getKafkaSasl(name)      -> {mechanism, username, password}   // or OAUTHBEARER token
```

`Secret`: `{ name, version, bytes (zeroizing), labels, createdMs, source, contentType }`.

## 8. Config schema (mirrors the streaming section)

```yaml
credentials:
  vault:
    path: "/greengrass/v2/work/{ComponentFullName}/vault"     # local store (template-resolved)
    keyProvider:
      type: "greengrass"        # greengrass | kms | file | env
      kmsKeyId: "arn:aws:kms:us-east-1:…:key/…"               # kms/greengrass
      region: "us-east-1"
      keyPath: "/etc/ggcommons/vault.key"                      # file
    keepVersions: 2
    cacheTtlSecs: 300
  central:
    type: "awsSecretsManager"   # awsSecretsManager | awsSsm | none
    region: "us-east-1"
    sync:
      secrets:  ["prod/db/password", "prod/kafka/sasl"]        # explicit, or…
      prefixes: ["myapp/"]                                      # …by prefix, or…
      tags:     { app: "myapp" }                                # …by tag filter
    refreshIntervalSecs: 300
    rotationGraceSecs: 600
    bootstrapOnStart: true
```

`central.type: none` → standalone local-only vault. Every numeric field uses the same
**lenient (float-tolerant) parsing** the streaming config now uses, since Greengrass delivers
config numbers as doubles.

## 9. Integration with existing subsystems

- **Streaming (fixes §7).** `KinesisSink`/`KafkaSink` stop owning credential logic; they
  request `getAwsCredentials(...)` / `getKafkaSasl(...)` from the credential service and
  re-fetch on a rotation listener. The streaming sink-credential config becomes a `secretRef`.
- **Messaging.** STANDALONE mTLS to IoT Core can pull its cert/key/CA via `getTlsBundle(...)`
  instead of plaintext file paths in the messaging config.
- **Config `secretRef` indirection (later).** A config value may be `{ "$secret": "name" }`,
  resolved **lazily at use-time** by the lib — so the secret is never substituted into the
  logged/templated config snapshot. Keeps secrets out of logs and shadow documents.
- **Observability.** A `CredentialMetricsBridge` (mirrors `StreamMetricsBridge`) emits
  non-sensitive metrics through the existing metric service: cached-secret count, last-sync
  age, sync failures, refreshes, decrypt failures, rotation events. **Never values.**
- **DI.** Registered as `ICredentialService` so tests inject an in-memory fake vault.

## 10. Security & threat model

- **At rest**: AEAD-encrypted; KEK never on disk in plaintext (KMS-wrapped or `0600` keyfile).
  AAD binds ciphertext to name+version+vaultId; vault MAC + monotonic versions resist
  record-swap and rollback.
- **In memory**: zeroized buffers, optional `mlock`, never logged/serialized.
- **Access control**: vault file `0600`, in the component work dir (ggc_user on GG). Cloud
  side: SM resource policy + the TES role + KMS key policy constrain which secrets a device
  can read and decrypt — selective sync is defense in depth.
- **Fail-closed**: KEK/KMS unavailable → vault stays locked, reads fail loudly (never fall
  back to plaintext). Corrupt vault / MAC mismatch → fail closed + alarm; optional re-bootstrap
  from central.
- **Audit**: access events (name + version + timestamp, **not value**) to the log/metric pipeline.
- **Blast radius**: per-component vaults + least-privilege sync mean a compromised component
  exposes only its own secrets.

## 11. Settled / open

**Settled (confirmed):** generic opaque secrets; offline-first; pull-only v1;
**per-language + normative format + cross-language test vectors (no native core)**; envelope
encryption with a pluggable `KeyProvider` that treats **HSM/TPM as a first-class custodian**;
KEK default on Greengrass = **KMS-via-TES with a keyfile fallback, HSM/TPM preferred when
present**; `gg.credentials()` interface; AWS Secrets Manager as the first central source.

**Rejected — shared native core (`ggvault` cdylib).** Tempting for one audited crypto impl
and an identical byte format, and it would reuse the streaming binding/packaging machinery.
Rejected because (a) credentials have no throughput driver that justified streaming's native
core, and (b) it would force a native-lib dependency onto every component that just wants a
secret — too heavy for a broadly-adopted primitive. Mature per-language crypto + a shared
spec + cross-language test vectors get the same byte-compatibility with lighter adoption.
(Revisit only if test-vector drift proves unmanageable.)

**Open / to confirm:**
- **Per-component vs shared device vault** as the default (recommend per-component; shared
  opt-in).
- **HSM/TPM interface**: standardize on **PKCS#11** as the one hardware abstraction (covers
  most HSMs, TPM 2.0 via a PKCS#11 shim, and many secure elements), vs adding a native TPM2
  path. Recommend PKCS#11-only to start. Confirm the target devices' hardware so phase 2 picks
  the right module.
- **Built-in per-language SDK central source vs a host-supplied fetch callback** as the default.
- Whether `config.secretRef` indirection is in scope early (keeps secrets out of logs/shadow)
  or deferred to phase 4.

## 12. Phasing

1. **Local vault core** (per language, against the §4 spec + test vectors): AEAD store,
   `FileKeyProvider`, get/put/list/delete/versions, change listeners, `gg.credentials()` +
   `ICredentialService` in all four libs. Standalone, local-only.
2. **AWS Secrets Manager central source + sync engine**: bootstrap, periodic + on-demand
   refresh, rotation grace, selective sync; **`KmsKeyProvider` / greengrass (TES)** envelope
   **and the `Pkcs11KeyProvider` (HSM/TPM)** — both land here so the hardware path is proven
   alongside KMS, not bolted on later. Offline-first + staleness metrics.
3. **Typed credential views** (AWS creds, basic-auth, TLS bundle, Kafka SASL) and **wire
   streaming + messaging to consume them** — closes `TELEMETRY_STREAMING.md` §7. Validate on
   the lab Nucleus with **real Secrets Manager + TES** (the leg streaming never exercised).
4. **Breadth**: SSM Parameter Store + HashiCorp/Azure sources, OS-keyring `KeyProvider`,
   `config.secretRef` indirection, audit log, optional push/bidirectional sync.
