# Platform model (platform × transport) — Requirements

> Companion to [README.md](README.md) and the design docs. **Status: PROPOSED.** Requirements use
> RFC-2119 keywords (**MUST** / **SHOULD** / **MAY**). IDs are stable handles for review and traceability.
> Each requirement carries an **Acceptance** note stating how it is verified.

## 0. Scope & non-goals

**In scope.** Adding a first-class **Kubernetes** deployment target to all four ggcommons libraries
(Java canonical; Python/Rust/TS mirrors) and the supporting packaging, by re-architecting the runtime
model into two orthogonal axes (platform × transport) and adding Kubernetes-native facilities to each
subsystem. Library + Helm packaging + an operator/CRD *sketch*.

**Target environments.** EKS Anywhere (EKSA) and generic self-managed CNCF clusters (k3s, RKE2,
OpenShift on-prem, kubeadm) at the **edge**, cooperating with cloud services over an **intermittent**
link. Components must tolerate lengthy cloud disconnects (offline-first / store-and-forward /
reconnect-and-resume); fully air-gapped is the supported extreme. EKS-in-cloud is supported only
incidentally.

**Non-goals.** (a) **EKS-in-cloud optimization** — supported incidentally, never the default;
(b) building a production Kubernetes Operator (only a CRD sketch); (c) CLI backward compatibility with
`-m` (explicitly dropped); (d) changing the on-wire message envelope, the config-schema *semantics*, or
the vault on-disk format/conformance vectors.

---

## 1. Functional requirements

### 1.1 Runtime model — platform × transport (FR-RT)

- **FR-RT-1 (two axes).** The libraries **MUST** replace the single `-m/--mode {GREENGRASS,STANDALONE}`
  selector with two independent selectors: `--platform {GREENGRASS,HOST,KUBERNETES,auto}` and
  `--transport {IPC,MQTT}`. *Acceptance:* CLI parsing accepts both flags in all four langs; `-m` is
  removed and rejected with a clear error message naming the replacements.
- **FR-RT-2 (platform-primary).** `--platform` **MUST** be the primary selector and `--transport`
  **MUST** default to a value **derived from the resolved platform** (GREENGRASS→IPC; HOST→MQTT;
  KUBERNETES→MQTT). *Acceptance:* omitting `--transport` yields the documented per-platform default;
  setting it overrides.
- **FR-RT-3 (precedence).** A profile **resolver** **MUST** apply settings under the precedence
  **explicit flag ▸ explicit config value ▸ platform-profile default ▸ library default** for every
  defaultable setting (config source, transport, metrics target, logging sink, credentials KeyProvider,
  parameters source, streaming buffer location, identity). *Acceptance:* a unit test per setting proves
  each precedence tier wins over the ones below it.
- **FR-RT-4 (auto-detect).** `--platform auto` (the default) **MUST** detect the platform from
  definitive signals — Nucleus IPC env (`AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT` / `SVCUID`)
  → GREENGRASS; service-account token (`/var/run/secrets/kubernetes.io/serviceaccount/token` or
  `KUBERNETES_SERVICE_HOST`) → KUBERNETES; else HOST — **MUST** be overridable by explicit `--platform`,
  and **MUST** log the detected platform and the basis for the decision. *Acceptance:* detection unit
  tests with each signal set/unset; an explicit flag always wins; the decision is logged at startup.
- **FR-RT-5 (invalid-combination guard).** The resolver **MUST** reject invalid (transport, platform)
  pairs at startup with a clear error — specifically `transport=IPC` with `platform∈{HOST,KUBERNETES}`
  (IPC requires the Nucleus). *Acceptance:* startup fails fast with a specific message; verified by test
  in all four langs.
- **FR-RT-6 (parse-time inputs only).** Platform/transport resolution **MUST** depend only on
  parse-time inputs (flags, environment, platform detection) and **MUST NOT** require the component
  config document (which loads after messaging). Any explicit `transport`/`platform` *config* override
  **MUST** be sourced from inputs available before messaging init (flags/env, or the messaging-config
  payload). *Acceptance:* the resolver runs before `init_messaging`; a test confirms no dependency on
  `ConfigManager` state.
- **FR-RT-7 (identity resolution).** Component/Thing identity **MUST** resolve in order: explicit
  `-t/--thing` ▸ platform-supplied identity (GREENGRASS: `AWS_IOT_THING_NAME`; KUBERNETES: Downward-API
  pod/namespace/annotation) ▸ library fallback. The resolved value **MUST** pass the existing
  template-variable sanitization. *Acceptance:* on k8s without `-t`, identity derives from Downward-API
  fields and `{ThingName}` substitution still works; sanitization test passes.
- **FR-RT-8 (builder parity).** The programmatic builders (`GGCommonsBuilder` / `GgCommonsBuilder` /
  TS/Rust equivalents) **MUST** expose platform/transport selection equivalent to the CLI. *Acceptance:*
  a component can select platform/transport without touching `argv`.

### 1.2 Config (FR-CFG)

- **FR-CFG-1 (CONFIGMAP source).** A new `-c CONFIGMAP [path]` source **MUST** read the component
  config from a mounted ConfigMap directory (default mount e.g. `/etc/ggcommons/config`, default key
  `config.json`) and **MUST** be the default config source on the KUBERNETES platform. *Acceptance:* a
  pod with a mounted ConfigMap loads config with no `-c` flag.
- **FR-CFG-2 (reuse FILE hot-reload seam).** The CONFIGMAP source **MUST** reuse the existing
  file-watch/`applyConfig` hot-reload path and **MUST** watch the *mount directory* (not the file
  inode) and **re-arm the watch after the kubelet's atomic `..data` symlink swap**. *Acceptance:* a
  ConfigMap edit propagates to a running pod (within kubelet sync latency) without restart, verified in
  all four langs; the watcher survives `IN_DELETE_SELF`.
- **FR-CFG-3 (subPath guard).** The CONFIGMAP source **MUST** document that `subPath` mounts never
  hot-reload and **SHOULD** detect and warn when it appears to be reading a `subPath` mount.
  *Acceptance:* docs state the restriction; a warning is emitted when reload cannot be guaranteed.
- **FR-CFG-4 (dotfile filter).** The CONFIGMAP source **MUST** ignore kubelet projection artifacts
  (`..data`, `..2026_*` timestamped dirs), reusing the filter already in `MountedDirSource`.
  *Acceptance:* `..`-prefixed entries are never parsed as config.
- **FR-CFG-5 (reject-and-keep).** On an invalid reload the CONFIGMAP source **MUST** keep the previous
  valid config (existing `applyConfig` behavior). *Acceptance:* a malformed ConfigMap edit does not
  crash a running pod.
- **FR-CFG-6 (Downward-API identity).** The libraries **SHOULD** support reading pod identity
  (`metadata.name`, `metadata.namespace`, node, and a `ggcommons.io/thing-name` annotation) from a
  Downward-API volume/env for FR-RT-7. *Acceptance:* identity derivation works from Downward-API inputs.

### 1.3 Messaging (FR-MSG)

- **FR-MSG-1 (config from active source).** On KUBERNETES the MQTT broker/TLS configuration **MUST** be
  sourced from the active config source (ConfigMap for endpoints + Secret for certs) rather than the
  positional `-m STANDALONE <file>` path, which is removed. *Acceptance:* a pod connects using broker
  config from a ConfigMap + certs from a mounted Secret, no positional path.
- **FR-MSG-2 (Service DNS).** Broker host configuration **MUST** accept a Kubernetes Service DNS name
  (e.g. `emqx.mqtt.svc.cluster.local`) for the local broker. *Acceptance:* a component resolves and
  connects to an in-cluster broker by Service DNS.
- **FR-MSG-3 (single- vs dual-MQTT).** The KUBERNETES profile **MUST** support both a single-broker
  topology (in-cluster broker only — required for air-gapped) and the dual-broker topology (local
  broker + IoT Core when egress exists); IoT Core **MUST** retain mutual TLS with SNI and no insecure
  fallback. *Acceptance:* both topologies connect; air-gapped config needs no IoT Core endpoint.
- **FR-MSG-4 (envelope unchanged).** The message envelope (header/tags/body, snake_case wire keys)
  **MUST** be byte-identical to today. *Acceptance:* the interop harness passes unchanged.

### 1.4 Metrics (FR-MET)

- **FR-MET-1 (prometheus target).** A new `prometheus` metric target **MUST** maintain an in-process
  registry and expose Prometheus/OpenMetrics text at an HTTP `/metrics` endpoint with a valid
  `Content-Type`. It **MUST** be the default metrics target on KUBERNETES. *Acceptance:* a Prometheus
  scrape of `/metrics` returns the component's metrics; Prometheus 3.x accepts the Content-Type.
- **FR-MET-2 (inverted lifecycle, documented).** For the `prometheus` target, `emitMetric` /
  `emitMetricNow` update the registry, `flush()` / `emitMetricNow()` are no-ops w.r.t. delivery, and
  `close()` stops the HTTP listener. This inversion **MUST** be documented and **MUST NOT** break the
  `MetricTarget` contract for other targets. *Acceptance:* docs state the no-op semantics; other
  targets unaffected; a test asserts `/metrics` reflects emitted values post-`flush`.
- **FR-MET-3 (dimension→label mapping).** A documented policy **MUST** map ggcommons dimensions
  (10-dimension cap, `coreName`/`largeFleetWorkaround`) onto Prometheus labels (the CloudWatch-isms
  have no Prometheus analog). *Acceptance:* the mapping is documented and implemented consistently.
- **FR-MET-4 (CloudWatch optional).** CloudWatch/EMF and the messaging targets **MUST** remain available
  (e.g. EMF-over-stdout) but **MUST NOT** be required; on air-gapped clusters the prometheus target
  **MUST** function with no AWS egress. *Acceptance:* metrics work with no AWS connectivity.
- **FR-MET-5 (CloudWatch durable buffering).** The Greengrass `cloudwatchcomponent` target **MUST NOT** be a
  profile default (retained for completeness only; it is unusable off-device); **direct `cloudwatch` is the
  preferred AWS push target**. Because the direct target buffers only in memory (`CloudWatch.java:30,108-204`),
  it **MUST** gain a **durable, disk-backed store-and-forward buffer that drains on reconnect**
  (NFR-DISCONNECT-1), implemented by **reusing the `ggstreamlog` durable log + export engine via a new
  host-callback sink** — design **resolved** in [../CLOUDWATCH_DURABLE_METRICS.md](../CLOUDWATCH_DURABLE_METRICS.md)
  (a **standalone enhancement, independent of this rearch** — it benefits today's GREENGRASS/STANDALONE
  edge equally; `buffer: durable|memory` runtime config, default `durable`; drop-stale-on-drain + counter;
  `dropOldest` retention). *Acceptance:* CloudWatch metrics survive a lengthy disconnect with **flat memory** and a
  disk-bounded backlog, drain cleanly on reconnect, and drop datums aged past CloudWatch's accept window
  with a nonzero `dropped_stale` counter.

### 1.5 Heartbeat & lifecycle (FR-HB)

- **FR-HB-1 (health endpoint).** An opt-in HTTP health server **MUST** expose `GET /livez`
  (process/event-loop alive; **MUST NOT** check external dependencies) and `GET /readyz` (200 only when
  messaging is connected and required subscriptions confirmed; 503 during startup and shutdown), and
  **SHOULD** expose `/startupz`. It is on by default on KUBERNETES. *Acceptance:* probes succeed/fail per
  spec; readiness flips to 503 immediately on SIGTERM.
- **FR-HB-2 (graceful shutdown on SIGTERM).** Components/libraries **MUST** wire SIGTERM to the existing
  `shutdown()` path that **unsubscribes every tracked subscription** and bounded-closes the runtime, in
  all four languages. *Acceptance:* on SIGTERM the component unsubscribes and exits 0 before the grace
  period; no subscription leak (does not trip the Nucleus shared-connection quota / reasonCode 151).
  This is the standing project rule, extended to k8s. (Note: GREENGRASS also receives SIGTERM on stop.)
- **FR-HB-3 (probe defaults provided).** The Helm chart **MUST** ship sensible probe defaults (startup
  gating slow connects; liveness not coupled to broker; readiness gating traffic). *Acceptance:* the
  rendered manifest contains working startup/liveness/readiness probes.
- **FR-HB-4 (container-aware metrics).** Heartbeat **SHOULD** report container/cgroup-aware resource
  usage (limits) rather than only host-level figures when running in a container. *Acceptance:* under a
  cgroup memory limit, reported memory reflects the limit, not the node. (May be deferred; see PARITY.)

### 1.6 Logging (FR-LOG)

- **FR-LOG-1 (stdout-JSON sink).** A structured **stdout-JSON** logging sink (one JSON object per line,
  no in-process file rotation) **MUST** be available and **MUST** be the default on KUBERNETES.
  *Acceptance:* logs emit one JSON object per line to stdout; file logging is off by default on k8s.
- **FR-LOG-2 (no in-process rotation).** On KUBERNETES the libraries **MUST NOT** perform in-process
  size-rotation by default (the cluster log agent owns rotation/retention). *Acceptance:* the
  `RollingFile`/equivalent appender is not installed under the k8s default.
- **FR-LOG-3 (correlation fields).** The stdout-JSON sink **SHOULD** include correlation fields
  (pod, namespace, node, thing) sourced from the Downward API. *Acceptance:* JSON lines carry the fields
  when supplied.
- **FR-LOG-4 (selected via existing token).** The sink **MUST** be selectable via the existing
  per-language `logging.<lang>_format` mechanism / a logging-format key, preserving the format-token
  contract. *Acceptance:* config selects the sink; non-k8s defaults (console/file) unchanged.

### 1.7 Credentials (FR-CRED)

- **FR-CRED-1 (SDK-chain auth, no code branches).** AWS authentication for the KeyProvider and central
  sync **MUST** continue to use the AWS SDK default credential provider chain with **no explicit
  credentials in ggcommons code**, so IRSA / IAM Roles Anywhere / static keys all work unchanged.
  *Acceptance:* a pod with an IRSA-bound ServiceAccount unlocks a KMS-backed vault with no code change.
- **FR-CRED-2 (local vault stays primary).** The encrypted local vault and its central-sync engine
  **MUST** remain available on all platforms; on air-gapped k8s the vault **MUST** function with no AWS
  connectivity (offline-first cache). *Acceptance:* vault open/get/put works with AWS unreachable when
  the KeyProvider is offline-capable (`file`/`pkcs11`/`env`).
- **FR-CRED-3 (env KeyProvider).** An `env` KeyProvider (KEK from an env var / mounted Secret) **MUST**
  be added (documented in `docs/CREDENTIALS.md` §5 but not implemented) — the k8s-idiomatic software-KEK.
  *Acceptance:* a vault unlocks from a KEK supplied via env/Secret.
- **FR-CRED-4 (optional CSI/ESO source).** An optional `CentralVaultSource` over a mounted-secret
  directory (the materialization shape of External Secrets Operator / Secrets Store CSI Driver) **MAY**
  be added so the operator owns cloud auth/rotation while ggcommons keeps typed views + `$secret`
  indirection. *Acceptance:* a component reads secrets the operator projected, via `gg.credentials()`,
  with no Secrets-Manager call from the pod.
- **FR-CRED-5 (shared-volume safety).** The libraries **MUST** document that the vault's advisory file
  lock is host/process-local and a vault on a `ReadWriteMany` volume **MUST NOT** be co-written by
  multiple pods. *Acceptance:* docs state per-pod vault (or single sync-owner) on k8s.
- **FR-CRED-6 (offline-capable KEK default).** The KUBERNETES profile's default vault `KeyProvider`
  **MUST** be offline-capable (`env` or `file`), not `kms`-only, so a pod cold-booting **during a cloud
  disconnect** can still unlock the vault (NFR-DISCONNECT-1). If `kms` is selected, an offline fallback
  KEK **MUST** be configurable (the code today picks exactly one provider with no automatic fallback —
  `Credentials.java:79-129`); a KMS-unreachable boot **MUST NOT** crash the component when a fallback is
  configured. *Acceptance:* a cold boot with the cloud severed unlocks the vault under the default
  KeyProvider; `kms`-without-fallback failing closed is documented as a risk (R10).

### 1.8 Parameters (FR-PARAM)

- **FR-PARAM-1 (mountedDir on ConfigMap/Secret).** The existing `mountedDir` source **MUST** be the
  first-class k8s parameters path (it already handles the `..data` dotfile farm and `securePaths`);
  it is the default on KUBERNETES. *Acceptance:* parameters load from a mounted ConfigMap/Secret with
  no cluster RBAC.
- **FR-PARAM-2 (SSM via chain).** `awsSsm` **MUST** work from a pod via the SDK default chain when
  reachable, with no explicit credentials. *Acceptance:* SSM fetch works under IRSA/IAM-Roles-Anywhere.
- **FR-PARAM-3 (cache durability).** The persistent encrypted cache **MUST** be documented as requiring
  a PersistentVolume to survive pod restart; on ephemeral storage the offline guarantee is lost and a
  cold start re-pulls from source. *Acceptance:* docs state the PV requirement for the persistent cache.
- **FR-PARAM-4 (secure mislabel guard).** Docs **MUST** warn that a Secret mounted without listing its
  subpath in `securePaths` is cached/surfaced unredacted. *Acceptance:* documented.

### 1.9 Streaming (FR-STREAM)

- **FR-STREAM-1 (PVC-backed durable buffer).** For a `disk` buffer carrying must-not-lose telemetry, the
  durable buffer directory (`buffer.path`) **MUST** sit on a PersistentVolume, and the workload **MUST**
  be a StatefulSet with a per-pod `volumeClaimTemplate` (or a single-replica Deployment + static PVC +
  `Recreate`). *Acceptance:* a pod reschedule preserves the unexported backlog; documented topology.
- **FR-STREAM-2 (single-writer).** The libraries **MUST** document and the packaging **MUST** enforce
  the single-writer-per-buffer invariant (`ReadWriteOncePod` where available; never `ReadWriteMany`
  shared writers). *Acceptance:* the chart uses RWO/RWOP; docs forbid shared-writer RWX.
- **FR-STREAM-3 (CSI-agnostic).** Storage guidance **MUST** be CSI-driver-agnostic via `StorageClass`
  (on-prem: local-path, Longhorn, Ceph/Rook, vSphere, NFS); EBS/EFS are mentioned only as the
  EKS-in-cloud case, not assumed. *Acceptance:* the chart parameterizes `storageClassName`; no
  EBS/EFS-specific assumption in defaults.
- **FR-STREAM-4 (lossless config recipe).** Docs **MUST** give the lossless k8s recipe: PVC +
  `onFull: block` (or generous `maxDiskBytes`) + `maxRetries: -1` + `fsync: always|perBatch`, sizing
  `maxDiskBytes ≤ PVC capacity`. *Acceptance:* documented and reflected in a chart values example.
- **FR-STREAM-5 (sink auth via chain).** Kinesis/Kafka sink AWS auth **MUST** use the SDK default chain
  (IRSA/IAM-Roles-Anywhere); `endpoint_url`/`region` remain config-overridable for private endpoints.
  *Acceptance:* Kinesis export works under IRSA with no explicit creds.
- **FR-STREAM-6 (graceful flush).** `terminationGracePeriodSeconds` **MUST** be sized to let
  `StreamService.close()` flush buffers and stop engines; a SIGKILL mid-batch **MUST** be safe
  (re-delivers on restart, at-least-once). *Acceptance:* graceful stop flushes; forced kill loses no
  committed data (only duplicates possible).
- **FR-STREAM-7 (native artifact matrix).** Each language **MUST** ship the correct
  `linux-x86_64`/`linux-aarch64` `ggstreamlog` artifact for the pod architecture (multi-arch images /
  per-arch wheels/prebuilds). *Acceptance:* `gg.streams()` works on both arches.
- **FR-STREAM-8 (back-pressure liveness safety).** Docs **MUST** state the `onFull: block` vs
  `dropOldest` durability/availability tradeoff: `block` guarantees no loss but **stalls the producing
  thread when the buffer fills during a lengthy disconnect**, which **MUST NOT** stall the liveness path
  (`/livez` **MUST NOT** be coupled to a blocked `append`, else a stall triggers a liveness-restart storm —
  the opposite of NFR-DISCONNECT-1). Sizing **MUST** target the worst-case disconnect window; where
  stalling is unacceptable, `dropOldest` (with counted drops) is the availability-first choice.
  *Acceptance:* documented tradeoff; `/livez` stays green while an `append` blocks on a full buffer.

### 1.10 Schema (FR-SCHEMA)

- **FR-SCHEMA-1 (additive sections).** New top-level sections **MUST** be added to the canonical
  `schema/ggcommons-config-schema.json` `properties{}` (top level is strict
  `additionalProperties:false`): `transport`, `platform`, `health` (probes), a `prometheus` branch in
  `metricEmission.targetConfig` + `"prometheus"` in the target enum, and an `identity` section.
  Existing semantics **MUST** be preserved (add, don't rename). *Acceptance:* existing valid configs
  still validate; new sections validate.
- **FR-SCHEMA-2 (sync gate).** Every schema change **MUST** be applied to the canonical file and synced
  via `schema/sync-schema.sh` into all five copies; `sync-schema.sh --check` **MUST** pass in CI.
  *Acceptance:* no schema drift; CI green.

### 1.11 Packaging (FR-PKG)

- **FR-PKG-1 (Helm chart).** A Helm chart **MUST** render the component workload (Deployment or
  StatefulSet), ConfigMap, Secret references, ServiceAccount + RBAC, probes, optional PVC, and a
  `ServiceMonitor`/`PodMonitor`, parameterized for edge/on-prem. *Acceptance:* `helm template` produces
  a deployable manifest set; `helm lint` passes.
- **FR-PKG-2 (compose existing operators).** The chart **MUST** integrate (not reimplement) External
  Secrets Operator and the Prometheus Operator where present, and **MUST** degrade gracefully when they
  are absent (e.g. plain Secret + no ServiceMonitor). *Acceptance:* installs with and without the
  operators present.
- **FR-PKG-3 (identity wiring).** The chart **MUST** support binding a ServiceAccount for IRSA/OIDC and
  mounting IAM-Roles-Anywhere material, and **MUST NOT** require EKS Pod Identity. *Acceptance:* values
  expose IRSA annotation and IAM-Roles-Anywhere mount; no Pod-Identity dependency.
- **FR-PKG-4 (NetworkPolicy default).** The chart **SHOULD** ship an optional egress NetworkPolicy
  hardening default that allows the broker + IoT Core ports, DNS, **and** the egress required by any
  enabled cloud subsystem (STS / KMS / Secrets Manager / SSM / Kinesis) — otherwise enabling it silently
  breaks IRSA/STS and cloud sync. These are dynamic IPs, so FQDN-based egress (Calico/Cilium) is needed.
  *Acceptance:* the opt-in NetworkPolicy renders and does not break cloud-cooperation paths for enabled
  subsystems.

### 1.12 Operator (FR-OP)

- **FR-OP-1 (no operator now).** The deliverable **MUST NOT** build a production operator; it **MUST**
  provide a `GgcommonsComponent` CRD *sketch* and document the explicit triggers that would justify one.
  *Acceptance:* DESIGN-operator.md contains the sketch + decision; no Go controller is shipped.

---

## 2. Non-functional requirements

- **NFR-COMPAT-1 (behavior preservation).** Phase 0 **MUST** preserve Greengrass *runtime behavior*;
  the existing test suites are the oracle. *Acceptance:* all pre-existing suites pass after Phase 0
  with tests re-pointed to the new CLI; on-device GREENGRASS behavior is unchanged.
- **NFR-COMPAT-2 (breaking CLI accepted).** Removing `-m` is an accepted pre-1.0 breaking change; legacy
  invocations **MUST** fail fast with guidance. *Acceptance:* `-m STANDALONE …` errors with a message
  pointing to `--platform/--transport`.
- **NFR-COMPAT-3 (data contracts).** The message envelope, vault on-disk format + conformance vectors,
  and config-schema semantics **MUST NOT** change (additive schema only). *Acceptance:* interop 32/32
  and vault vectors pass unchanged.
- **NFR-PARITY-1 (four-way).** Every public behavior change **MUST** land in all four languages or be
  explicitly deferred with a tracked parity note (Java canonical first). *Acceptance:* parity register
  updated; no silent divergence.
- **NFR-PORT-1 (vanilla k8s).** All k8s features **MUST** be *operable* on a generic CNCF cluster with no
  AWS dependency; AWS integrations are an **expected-but-intermittent cooperation layer**, not a baseline
  requirement (a cluster with no AWS reachability still runs components fully). *Acceptance:* the full
  k8s feature set is exercised on a non-AWS cluster (e.g. k3s/kind) in CI or a documented manual run.
- **NFR-DISCONNECT-1 (lengthy-disconnect tolerance).** Every cloud-dependent subsystem **MUST** tolerate
  lengthy disconnects from cloud services without failing the component: reads served from offline-first
  caches (vault, parameters), telemetry stored-and-forwarded (streaming), last-known values retained on
  sync failure, local pub/sub continued via the in-cluster broker, and automatic resume on reconnect. A
  cloud/AWS call failure (expired STS creds, unreachable endpoint) **MUST NOT** crash the component.
  *Acceptance:* a fault-injection test that severs cloud connectivity for an extended period leaves the
  component running and serving cached data, and it resumes cleanly on reconnect.
- **NFR-PORT-2 (air-gapped extreme).** A fully air-gapped deployment **MUST** be possible as the extreme
  of NFR-DISCONNECT-1: in-cluster broker only, offline KeyProvider, `mountedDir` parameters, local
  durable buffer — no cloud reachability required at all. *Acceptance:* documented air-gapped profile;
  zero outbound cloud calls when so configured.
- **NFR-SEC-1 (no creds in code).** No ggcommons code path **MUST** embed or require explicit AWS
  credentials. *Acceptance:* code review + grep; all SDK clients use the default chain.
- **NFR-SEC-2 (least privilege).** RBAC and IAM artifacts the chart ships **MUST** be least-privilege
  (no cluster-admin; scoped Service/ConfigMap/Secret access only where required). *Acceptance:* rendered
  RBAC is namespaced and minimal; documented.
- **NFR-SEC-3 (secret handling).** Secrets **SHOULD** be consumed as mounted files (tmpfs) over env
  vars; **KEK/PIN material MUST come from a Secret/tmpfs mount, never a ConfigMap or chart
  `values.yaml`**; and the libraries **MUST NOT** log secret values — the new structured stdout-JSON
  context-field path **MUST** route through the existing `Secret` redaction (Downward-API correlation
  fields are non-sensitive by construction). *Acceptance:* redaction tests pass (incl. structured
  fields); docs prefer file mounts; no KEK/PIN material in ConfigMaps or chart values.
- **NFR-SEC-4 (listening surface).** The new inbound HTTP listeners (`/metrics`, `/livez`, `/readyz`)
  **MUST** bind only declared ports, **SHOULD** be separable from business ports, and **SHOULD** default
  their bind address to the pod IP (not `0.0.0.0`) with a configurable `bindAddress`. `/metrics` carries
  identifiers (thing/topic/dimension names) but **no secret values**, and in-cluster scrape is
  intentionally unauthenticated (protected by NetworkPolicy + the ServiceMonitor selector); `/livez`/
  `/readyz` expose only connection state. *Acceptance:* ports + bind address are configurable and
  declared as container ports; the no-secrets-in-`/metrics` property is documented.
- **NFR-OBS-1 (self-observability).** Streaming backlog/drops, credential/parameter sync stats, and
  reload events **SHOULD** be surfaced via metrics/heartbeat so operators can alert. *Acceptance:*
  `stats()` values are exposed via the active metric target.
- **NFR-FOOT-1 (footprint).** Added listeners/threads (HTTP health server, prometheus registry) **MUST**
  be lightweight and opt-out, suitable for constrained edge pods. *Acceptance:* documented overhead;
  features disable cleanly.
- **NFR-DOC-1 (docs).** Each subsystem's `doc/`/`docs/` page and `CLAUDE.md` (CLI contract) **MUST** be
  updated; the `docs/platform/` set documents the platform model and deployment. *Acceptance:* docs
  updated in the same change as behavior.

---

## 3. Verification matrix (summary)

| Area | Primary verification |
|------|----------------------|
| Resolver & precedence | Unit tests per setting/precedence tier; invalid-combo guard test (FR-RT-3/5) |
| Auto-detect | Signal-matrix unit tests; startup log assertion (FR-RT-4) |
| Greengrass behavior | Full pre-existing suites green after Phase 0 (NFR-COMPAT-1) |
| Config hot-reload | ConfigMap-edit propagation test incl. `..data` swap re-arm, all 4 langs (FR-CFG-2) |
| Prometheus target | Scrape returns metrics; lifecycle no-op test (FR-MET-1/2) |
| Probes & shutdown | SIGTERM unsubscribe test; probe pass/fail behavior (FR-HB-1/2) |
| Streaming durability | Reschedule-preserves-backlog test on PVC; emptyDir-loses test (FR-STREAM-1) |
| AWS auth | IRSA + IAM-Roles-Anywhere integration; air-gapped offline run (FR-CRED-1, NFR-PORT-2) |
| Schema | Existing+new config validate; `sync-schema.sh --check` green (FR-SCHEMA) |
| Interop / vault | Interop 32/32 and vault vectors unchanged (NFR-COMPAT-3) |
| Packaging | `helm lint`/`template`; install with/without ESO+Prometheus operators (FR-PKG) |
| Operator | Doc-only: DESIGN-operator contains the CRD sketch + recommendation (FR-OP-1) |
| Self-observability | streaming/credentials/parameters `stats()` + reload events surfaced via the active metric target (NFR-OBS-1) |
| Disconnect tolerance | fault-injection severs cloud connectivity; component keeps serving cached data and resumes (NFR-DISCONNECT-1) |

## 4. Risk register

| # | Risk | Severity | Mitigation |
|---|------|----------|------------|
| R1 | Four-language rearchitecture of the init/CLI path regresses the proven Greengrass path | High | Phase 0 "behavior unchanged" oracle; Java-canonical-first; full suite as gate |
| R2 | ConfigMap hot-reload silently dead (`subPath`; watcher not re-arming after `..data` swap) | High | FR-CFG-2/3; per-lang watcher re-arm tests; loud subPath docs/warn |
| R3 | Telemetry loss when durable buffer on ephemeral storage | High | FR-STREAM-1/4; StatefulSet+PVC; lossless recipe; no emptyDir default for `disk` |
| R4 | Prometheus pull lifecycle inversion breaks flush-before-exit expectations | Med | FR-MET-2 explicit docs; keep other targets' semantics intact |
| R5 | Init-order circularity blocks config-derived transport/platform overrides | Med | FR-RT-6; resolver on parse-time inputs only; document the constraint |
| R6 | Schema drift across the 6-file sync | Med | FR-SCHEMA-2; CI drift gate |
| R7 | Parity drift (8 subsystems × 4 langs); logging/metrics have no shared sink seam | Med | NFR-PARITY-1; per-subsystem parity notes in PARITY.md |
| R8 | New inbound listeners expand attack surface on edge pods | Med | NFR-SEC-4; opt-out; declared ports; **default bind to pod IP (not 0.0.0.0)**; NetworkPolicy + ServiceMonitor selector |
| R9 | `endpointOverride`/static keys accidentally shipped to prod | Low | NFR-SEC-1; review; document floci/LocalStack-only use |
| R10 | KMS-only KeyProvider fails closed on a disconnected cold boot → component can't unlock the vault and crashes | High | FR-CRED-6: offline-capable KEK default (`env`/`file`); configurable KMS fallback |
| R11 | `onFull: block` back-pressure stalls the producer / trips liveness during a full-buffer disconnect | Med-High | FR-STREAM-8: size for worst-case window; decouple `/livez` from a blocked `append`; `dropOldest` where availability-first |
| R12 | MQTT/mTLS cert rotation needs a pod restart (certs load once at init) | Med | roll pods on rotation (Reloader); documented known gap, not fixed in this phase |
