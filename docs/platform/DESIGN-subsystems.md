# Design — Subsystems on Kubernetes

> Companion to [DESIGN-core.md](DESIGN-core.md). **Status: PROPOSED.** Per-subsystem design for all
> eight, each as: **Seam today → Kubernetes addition → Config → Disconnect tolerance → Parity → Risks.**
> The KUBERNETES profile defaults are summarized in DESIGN-core §3. Connectivity model throughout:
> **edge-first with intermittent cloud cooperation** — cloud is used where appropriate, but every
> cloud-dependent path tolerates lengthy disconnects. `file:line` cites the current tree.

---

## 1. Config — a `CONFIGMAP` source

**Seam today.** `ConfigProvider` is a *sealed* base (`ConfigProvider.java:11-26`) permitting
FILE/ENV/GG_CONFIG/SHADOW/CONFIG_COMPONENT; selection is a string switch in `ConfigProviderBuilder`
(`:18`). FILE reloads via a daemon `FileWatcher` that watches the **parent directory** for
MODIFY+CREATE and calls `applyConfig` (`FileConfigProvider.java:74-80`, `FileWatcher.java:86-91`).
Reload re-validates and **rejects-and-keeps** the previous config on failure (`ConfigManager.java:99-107`).

**Kubernetes addition.** Add `-c CONFIGMAP [mountPath]` (default `/etc/ggcommons/config`, default key
`config.json`), the canonical analogue of FILE, **reusing the FILE hot-reload seam**. It is the default
config source on KUBERNETES. Do **not** add an env-var-only k8s source — `ENV` already covers static
config and cannot hot-reload.

The single hard requirement is the **kubelet atomic `..data` symlink swap**: the watcher must watch the
*mount directory* (it already does) and **re-arm after `IN_DELETE_SELF`/move**, then re-read. An inotify
watch on the user-visible file alone fires once and dies. This must be verified in all four watchers
(Java `WatchService`, Python watchdog/inotify, Rust `notify`, and the Node watcher) — the highest-risk
parity item. In particular Node `fs.watch` is inode-based and commonly **stops delivering events after
the `..data` rename**; the TS impl likely needs to watch the *directory* and re-add the watch on
`rename`/delete, or switch to `chokidar`/polling — do not assume `fs.watch` already has the correct shape. Reuse the `MountedDirSource` dotfile filter (`MountedDirSource.java:54-57`) to skip `..data` /
`..2026_*`.

**The `subPath` gotcha:** a ConfigMap mounted with `subPath` is **never** updated by the kubelet —
hot-reload is silently dead. The provider must document "mount the whole volume, not a `subPath`" and
**SHOULD warn** when it cannot guarantee reload. Ops fallback for forced `subPath`/env/immutable: a
restart-on-change controller (Stakater Reloader, `reloader.stakater.com/auto: "true"`).

**Config / schema.** Sealed hierarchy: add the `permits` entry (`ConfigProvider.java:12`) + a builder
`case`; Rust/TS add a `ConfigSource` impl (open seam). Identity from the Downward API feeds
`{ThingName}`/`{ComponentName}` (DESIGN-core §6.2).

**Disconnect tolerance.** ConfigMap is local (kubelet-projected); no cloud dependency. Reject-and-keep
means a bad edit never crashes a running pod. Propagation is ~60–90 s at kubelet defaults (sync period +
cache TTL) — acceptable for config; document `configMapAndSecretChangeDetectionStrategy: Get` only as a
rare low-latency exception.

**Parity.** Source set exists in all four (`config/source/*` in Rust/TS, `config/manager/*` in Python).
Sanitization + template rules are a documented cross-language contract — unchanged.

**Risks.** Silent reload death via `subPath`/non-re-arming watcher (R2); WatchService coalescing rapid
edits (benign for ConfigMap swaps); identity falling back to `NOT_GREENGRASS` if Downward-API not wired.

---

## 2. Messaging — broker via Service DNS; config from ConfigMap+Secret

**Seam today.** `MessagingClient` chooses the provider once at init from `mode`
(`MessagingClient.java:42-61`): GREENGRASS→IPC; STANDALONE→`StandaloneMessagingProvider` with a
`MessagingConfiguration.loadFromFile(<positional path>)` holding a **required `local`** broker + optional
`iotCore` (mutual TLS, no insecure fallback). The wire envelope (header/tags/body, snake_case keys) is
identical across languages and **must not change**.

**Kubernetes addition (no provider rewrite — only how config is supplied and what brokers point at).**
1. **Drop the positional path.** Source MQTT config from the active config source: endpoints/ports/clientIDs
   from a **ConfigMap**, certs/passwords from a mounted **Secret** (paths like `/etc/ggcommons/certs`).
   The existing `certPath/keyPath/caPath` fields work unchanged against the Secret mount; reuse the
   `-c ENV`-style idea so a messaging config can also come from an env/Secret.
2. **Service DNS.** `local.host` becomes an in-cluster Service DNS name (e.g.
   `emqx.mqtt.svc.cluster.local:1883`) instead of `localhost`/a fixed file value.
3. **dual-MQTT is the natural edge default.** Because the target is *edge with cloud cooperation*, the
   dual model genuinely earns its keep: the **in-cluster broker** carries local pub/sub and **stays up
   when IoT Core is unreachable**, while **IoT Core** (ATS endpoint, mTLS 8883 with SNI; 443+ALPN
   `x-amzn-mqtt-ca` when egress is 443-only) provides cloud cooperation. Single-MQTT remains an option:
   cloud-only (no local fanout) or local-only (air-gapped extreme).

**Config / schema.** New `transport` section (`type: dualMqtt|iotCoreOnly|localBrokerOnly|ipc`) that
**references** the existing `messaging` section (`$ref` to `definitions.mqttBroker`) — *transport
selects, messaging configures* (DESIGN-core §8). Existing STANDALONE `messaging` configs keep validating.

**Disconnect tolerance (central).** This is the subsystem where the connectivity model bites hardest:
- A dropped IoT Core link **MUST NOT** stop local pub/sub — the in-cluster broker keeps serving.
- The provider **MUST** reconnect-and-resume to IoT Core on link return (set the MQTT keep-alive below the
  interface-endpoint/NLB **idle timeout** — deployment-specific, commonly ~350 s — when reaching IoT Core
  via a PrivateLink interface VPC endpoint on private clusters).
- For private clusters, support pointing `iotCore.endpoint` at an interface VPC endpoint
  (PrivateLink) so the cloud path doesn't depend on NAT egress.
- Known gaps to track: TLS certs load once at init (rotation needs restart); no per-call timeout in
  Java/Python `request()` (Rust/TS have `timeoutMs`). Flagged, not fixed here.

**Parity.** Rust/TS have a `MessagingService`/`IMessagingService` trait seam; Java/Python wire concrete
providers (no service interface) — config-supply change is localized in all four. `receiveOwnMessages`
default diverges (TS `false`, others `true`) — carry as a tracked pre-existing gap.

**Risks.** Broker host hardcoding (mitigated by Service DNS); cert rotation requires restart; late/dup
replies after a future completes (handled variably across langs). NetworkPolicy egress hardening needs a
policy-capable CNI (Calico/Cilium FQDN egress easiest for the DNS endpoint).

---

## 3. Metrics — a pull-based `prometheus` target

**Seam today.** `MetricTarget` is a *sealed* base permitting `{CloudWatch, CloudWatchComponent, Messaging,
Log}` (`MetricTarget.java:16-18`); selection is a string switch in `MetricEmitter` keyed on
`metricConfig.getTarget()` (`MetricEmitter.java:52-74`), default `log`. Every existing target is **push**.
EMF JSON is built by `EmfHelper` and reused by Log/Messaging; CloudWatch uses the SDK `PutMetricData`.

**Kubernetes addition.** Add a `prometheus` target — the default on KUBERNETES — that **inverts the push
lifecycle**: `emitMetric`/`emitMetricNow` update an in-process registry (counter/gauge/histogram per
metric/measure/dimension-set); a tiny embedded HTTP server serves OpenMetrics text at `/metrics` with a
valid `Content-Type` (Prometheus 3.x rejects a missing/blank type); `flush()`/`emitMetricNow()` become
**no-ops w.r.t. delivery**; `close()` stops the listener. Backed by the maintained client per language:
`client_java` (official), `client_python` (official), the **community** `prometheus-client` crate for
Rust (behind a new off-by-default `metrics-prometheus` cargo feature, matching the `cloudwatch` pattern —
there is no Prometheus-org official Rust client), and `prom-client` (the community Node lib — no official
option).

Scrape wiring is a **ServiceMonitor** (or PodMonitor for Job/CronJob/sidecar pods) — vendor-neutral,
runs on any cluster with the Prometheus Operator (DESIGN-packaging). Keep CloudWatch reachable without a
second code path via **EMF-over-stdout** (trivial `Log`-target variant writing `EmfHelper` JSON to stdout
for Fluent Bit/Container Insights) or an ADOT Collector scraping `/metrics` → AMP remote-write — both
optional, connectivity-gated.

**Direct CloudWatch — the preferred AWS push target, with a durable buffer (FR-MET-5; design resolved in [../CLOUDWATCH_DURABLE_METRICS.md](../CLOUDWATCH_DURABLE_METRICS.md) — a standalone enhancement, independent of this rearch).**
When a component *does* push to CloudWatch, the **direct `cloudwatch` target is preferred**; the Greengrass
`cloudwatchcomponent` target is retained **for completeness only and is never a profile default** (and
GREENGRASS/HOST keep the current `log` library default — DESIGN-core §3). But the direct target today
batches **in memory** and flushes on a timer via `PutMetricData` (`CloudWatch.java:30,108-204`) — on an
intermittently-connected edge fleet a lengthy disconnect makes that in-memory batch grow unbounded or drop,
violating NFR-DISCONNECT-1. **Approach (resolved):** give the direct CloudWatch target a **durable,
disk-backed store-and-forward buffer that drains `PutMetricData` on reconnect** by **reusing the
`ggstreamlog` durable log + export engine via a host-callback sink** — the core keeps the buffer + the
at-least-once export loop; the CloudWatch send (datum build + `PutMetricData`) stays in the metrics layer
(reusing the existing per-language SDK client), so nothing is duplicated in the Rust core. `buffer:
durable|memory` is a **runtime** config choice (default `durable`); stale datums outside CloudWatch's
~2wk/~2h window are **dropped + counted** on drain (retry-forever can't fix an aged-out timestamp);
retention is `dropOldest`. Full design + ABI delta + per-language plan in
[../CLOUDWATCH_DURABLE_METRICS.md](../CLOUDWATCH_DURABLE_METRICS.md). (`cloudwatchcomponent` is
off-device-unusable anyway — it needs the Nucleus component — reinforcing "completeness only".)

**Config / schema.** Append `"prometheus"` to the `metricEmission.target` enum (`:87`); add a `prometheus`
branch (`port`, `path`) to `targetConfig` (which is `additionalProperties:false`, `:128`).

**Disconnect tolerance.** Pull is inherently disconnect-tolerant for the *edge*: no client-side egress or
creds; a missed scrape is just a gap, and Prometheus gets a free `up` liveness signal. CloudWatch/AMP push
is optional and may lapse during a disconnect — with no impact on the component *if* the direct CloudWatch
target gains the durable buffer above (else its in-memory batch is the loss/growth risk). **Self-observability
(NFR-OBS-1):** streaming `stats()` (backlog, `oldest_unacked_age_ms`, `dropped_total`, retries),
credential/parameter sync counters, and config-reload events are exposed via the active metric target so
operators can alert on a growing backlog or sync failures during a disconnect.

**Parity.** Four-way target set + EMF envelope + default-to-`log`-on-unknown preserved. Document the
lifecycle inversion uniformly. Java/Python silently skip messaging injection (NPE risk); TS/Rust hard-error
— pre-existing, note it.

**Risks (R4).** A caller relying on `emitMetricNow` for flush-before-exit gets **nothing** until the next
scrape under the prometheus target — must be documented. New inbound listener/port (NFR-SEC-4). Dimension
cap (10) and `coreName`/`largeFleetWorkaround` are CloudWatch-isms needing a documented label-mapping policy.

### 3.1 Edge networking & central aggregation (how pull works behind a "no cloud-inbound" firewall)

The pull model does **not** mean the cloud reaches into the edge. `/metrics` is only ever scraped from
**inside the site/cluster trust boundary** — a collector (sidecar / per-node agent / per-site Prometheus)
on the same network as the pod. The cloud never initiates a connection to the edge, so a "no inbound HTTP
from the cloud" posture is satisfied by construction. Central, multi-site aggregation is achieved with
**outbound, edge-initiated `remote_write`/push** from a per-site collector to a central store:

```
  cloud ──X──▶ (no inbound)   SITE / FACTORY (edge cluster)
                              component pods ──/metrics (intra-site scrape)──▶ local collector
                                                                              (Alloy / ADOT / vmagent /
                                                                               Prometheus-agent)
                                                                                   │ remote_write (HTTPS 443, OUTBOUND)
                                                                                   ▼
                                                  central TSDB (AMP / Mimir / Thanos / VictoriaMetrics / Grafana Cloud)
```

- **Same network direction as CloudWatch** (edge→cloud egress); only the protocol differs (`remote_write`
  vs `PutMetricData`). The egress moves from *inside each component* to *one collector per site*.
- **Topology labels** (`site`/`factory`, `cluster`, `node`, plus the component's `thing`/`component`
  dimensions) are the multi-tenancy keys — the Prometheus equivalent of CloudWatch dimensions; aggregate
  centrally with `sum by (site, component) (...)` and federate **edge → regional → central** for large fleets.
- **Disconnect tolerance**: the collector's **WAL** buffers during a cloud outage and drains on reconnect
  (the pull analogue of the FR-MET-5 durable CloudWatch buffer); the **component** has zero cloud
  dependency/egress for metrics, and a missed scrape is just a gap (Prometheus also gets a free `up`
  liveness signal). A fully **air-gapped** site still gets complete *local* observability (on-site
  Prometheus/Grafana) and catches up centrally if/when a link exists.
- **AWS/CloudWatch parity**, all outbound: (1) keep the direct `cloudwatch` durable-buffer target
  (FR-MET-5) as the literal CloudWatch path; (2) an **ADOT Collector** scrapes `/metrics` → AMP
  remote_write or → CloudWatch (EMF exporter); (3) **EMF-over-stdout** → Fluent Bit → CloudWatch Logs /
  Container Insights. The prometheus target is **additive** to the push targets, never a replacement.

The Helm chart ships an **opt-in `ServiceMonitor`** (off by default; requires the Prometheus Operator)
documenting the in-cluster scrape; the smoke validates `/metrics` by a direct in-pod GET instead.

### 3.2 Deferred enhancements (captured, not built in the prometheus slice)

The shipped target is the standard **in-memory gauge** registry (latest-value per measure). Three
follow-ups are recorded for a deliberate later decision:

- **(B) Measure `type` model (gauge / counter / histogram).** ggcommons measures carry name+unit but no
  type, so every measure maps to a latest-value **gauge** — which is lossy *between scrapes* for
  measurement-style values (e.g. latency). The idiomatic Prometheus fix is to model a measure type and map
  measurements to **histograms** (every observation contributes to count/sum/buckets, surviving scrape
  gaps in aggregate) and monotonic values to **counters**. This is a four-language `Metric`/`Measure` API
  addition — do it in lockstep, Java canonical.
- **(C) Unified durable "metrics streamlog → pluggable exporters".** Have every `emitMetric` append to one
  durable `ggstreamlog` metrics buffer, with each target a reader/exporter (CloudWatch/Kinesis drain+push;
  prometheus folds the log into its registry). Wins: one durable buffer for push *and* pull + counter
  **restart-survival**. It should **subsume the FR-MET-5 CloudWatch buffer**; bigger, best as its own phase.
  Note: for the *pull* path this does **not** add lossless-between-scrapes (the scraper still samples — that
  durability is the collector WAL's job); its value is restart-durability + unifying push/pull behind one buffer.
- **(D) Heartbeat in a pull world.** The heartbeat is a metric *producer* that already routes through the
  metric target (so on KUBERNETES its samples land in the prometheus registry — not "folded in"). But its
  internal **interval timer is partly redundant with the scrape interval**; the idiomatic pull pattern is a
  **scrape-time collector** (sample lazily when pulled). And k8s already exposes per-container CPU/mem via
  cAdvisor/kubelet, so per-pod heartbeat resource metrics are partly redundant at the infra layer (app-level
  heartbeat still adds self-reported liveness + the `thing` identity + off-k8s value). Ties into FR-HB-4
  (cgroup-aware heartbeat).

---

## 4. Heartbeat — an HTTP health endpoint + graceful shutdown

**Seam today.** Heartbeat samples CPU/mem/disk/threads/FDs on an interval and routes flattened stats to a
metric or messaging target via a string switch (`Heartbeat.publishHeartbeat()` Java `:125-173`). There is
**no HTTP health endpoint** and **no auto-wired SIGTERM** — `GGCommons.shutdown()` exists but the app must
call it from its own signal handler.

**Kubernetes addition.**
1. **HTTP health server** (opt-in `health` config section; on by default on KUBERNETES):
   - `GET /livez` → 200 while the process/event-loop/heartbeat tick is alive. **MUST NOT** check the
     broker (a broker outage must not cause liveness restart storms).
   - `GET /readyz` → 200 only when messaging is connected and required subscriptions confirmed; 503
     during startup and shutdown.
   - optional `GET /startupz` for slow connects (else reuse `/readyz` with a generous `failureThreshold`).
   Handlers are allocation-free, dependency-free, minimal-body (per probe guidance). HTTP not gRPC/exec
   for portability across all four languages.
2. **SIGTERM → graceful shutdown (the hard requirement, FR-HB-2).** Wire the kubelet's SIGTERM to the
   existing `shutdown()` that unsubscribes every tracked subscription and bounded-closes the runtime, in
   all four langs (Java `Runtime.addShutdownHook`; Python `signal.signal(SIGTERM,…)`; Rust
   `tokio::signal::unix` terminate; Node `process.on('SIGTERM')` — already the TS pattern). On SIGTERM:
   flip `/readyz` to 503 → unsubscribe all → close messaging/streams/vault → exit 0. This prevents
   leaking subscriptions onto the Nucleus shared MQTT connection and tripping `QUOTA_EXCEEDED`
   (reasonCode 151) — the standing project rule, and it matters in GREENGRASS too (the Nucleus also sends
   SIGTERM). See [[unsubscribe-before-exit]].

**Config / schema.** New `definitions.healthProbes` + `properties.health` (`liveness/readiness/startup`
`{path,port,intervalSecs}` + enable toggle). Recommended manifest probe block in DESIGN-packaging.

**Disconnect tolerance.** `/livez` deliberately excludes the broker so a cloud (or even local-broker)
outage never triggers a restart; `/readyz` reflects connectivity so traffic is gated but the pod is not
killed. Heartbeat continues sampling and buffering locally during disconnects. (Ordering note: endpoint
removal is **deletion-driven** — the EndpointSlice flips to not-ready when the pod is marked Terminating —
and `preStop` runs *before* SIGTERM, so the `/readyz`→503 on SIGTERM is a belt-and-suspenders signal that
fires post-`preStop`, not the primary drain mechanism.)

**Parity.** New public behavior + a new config section → Java canonical first, then Python/Rust/TS; add to
the schema + `sync-schema`. Heartbeat monitors are OSHI/psutil/sysinfo respectively (units already aligned).

**Risks.** Without SIGTERM wiring, a SIGKILL at grace-period end leaks subscriptions (the exact bug the
rule guards). Container/cgroup-aware metrics (FR-HB-4) — host-level sysinfo over-reports under limits;
may be deferred. New inbound port (NFR-SEC-4).

---

## 5. Logging — a stdout-JSON sink

**Seam today.** A facade (`LoggerFactory` auto-detects SLF4J→Log4j2→JUL) plus a config-driven Log4j2
configurator that always installs a `Console` (`SYSTEM_OUT`) appender and an optional **size-rotated
`RollingFile`** appender (`ConfigManager.java:435-466`). There is **no pluggable sink abstraction** —
sinks are hard-coded console + rolling file; the per-language `logging.<lang>_format` token is honored.

**Kubernetes addition.** Add a **stdout-JSON** sink (one JSON object per line, e.g. Log4j2
`JsonTemplateLayout` on the Console appender) — the default on KUBERNETES — and **disable in-process file
rotation** (the cluster log agent owns rotation/retention; in-process rotation fights the platform and can
make agents miss/double-count rotated content). The always-on `SYSTEM_OUT` appender is already correct;
only the layout (JSON) and disabling the `RollingFile` appender change. Add optional **correlation fields**
(pod/namespace/node/thing) from the Downward API via MDC/ThreadContext + the JSON layout's extra fields.

**Config / schema.** Selected via the existing `logging.<lang>_format` mechanism / a logging-format key —
no new top-level section required; non-k8s defaults (console + file) unchanged.

**Disconnect tolerance.** Stdout has no cloud dependency; the cluster agent (Fluent Bit → CloudWatch/Loki/
etc.) buffers and ships. With `log_format emf`, the same stdout stream can also carry EMF metrics. Cloud
log-sink outages are the agent's problem, not the component's.

**Parity.** **Java-only today:** the framework-detection facade, `globalControl` whole-app takeover, and
the isolated namespace configurator. The stdout-JSON sink + correlation fields are **greenfield in all
four** and must be added in lockstep (no shared sink seam → high parity-drift risk). Rust cannot
reconfigure tracing layers after install (`logging.rs:24-29`) — format/sink fixed at install; note the gap.

**Risks.** No sink abstraction → four configurators edited in lockstep (R7). `globalControl` can destroy a
host app's logging if misenabled. Errors swallowed to stderr. Read-only root FS breaks file appender (moot
once file logging is off on k8s).

---

## 6. Credentials — keep the vault; SDK-chain auth; optional ESO/CSI

**Seam today.** An encrypted local **vault** (AES-256-GCM envelope; per-vault DEK wrapped by a pluggable
`KeyProvider` KEK; HKDF MAC over the set; AAD binding) with optional **central sync** from AWS Secrets
Manager (`SyncEngine`, offline-first: a fetch failure keeps the cached value). **Crucially, both AWS clients
(`KmsKeyProvider`, `AwsSecretsManagerSource`) are built with no explicit credentials** — they rely on the
**SDK default credential provider chain** (`KmsKeyProvider.java:34-44`, `AwsSecretsManagerSource.java:22-31`),
which resolves TES on Greengrass and the normal chain elsewhere. KeyProviders: `file`, `kms`/`greengrass`,
`pkcs11`. On-disk format is normative and policed by `vault-test-vectors/` — **must not change**.

**Kubernetes addition (mostly config + one small KeyProvider, very little code).**
1. **AWS auth "just works" via the chain.** IRSA (web-identity token) and IAM Roles Anywhere are picked up
   automatically — same mechanism as TES. Selecting `keyProvider.type: kms` + `central.type:
   awsSecretsManager` in a pod with the right identity is sufficient; **no new provider class** (FR-CRED-1).
2. **Add an `env` KeyProvider** (KEK base64 from an env var / mounted Secret) — documented in
   `docs/CREDENTIALS.md` §5 but not implemented; the k8s-idiomatic software-KEK (FR-CRED-3).
3. **Optional `CentralVaultSource` over a mounted-secret directory** (the materialization shape of ESO /
   Secrets Store CSI Driver) — `Credentials.open` currently hard-rejects any `central.type` other than
   `none`/`awsSecretsManager` (`Credentials.java:49-51`); adding a `mountedDir`/`k8sSecret` arm is localized
   and mirrors how `parameters` already does `mountedDir`. Lets the operator own cloud auth/rotation while
   ggcommons keeps typed views + `$secret` indirection (FR-CRED-4).

**Keep the vault.** On modern EKS, k8s Secrets are KMS-encrypted at rest and ESO/CSI can subsume central
sync — but the target is **edge with intermittent connectivity**, so the vault's value is real and primary:
(a) **offline-first durable cache** across lengthy disconnects (ESO/CSI assume reachable AWS and degrade);
(b) **runtime/dynamic** programmatic fetch (operators are deploy/poll-time only); (c) **API portability** —
one `gg.credentials()` across edge and k8s. On k8s you may back the accessor with the `env`/`mountedDir`
providers and let EKS KMS-at-rest cover encryption, but the vault stays the offline-first engine.

**Config / schema.** New `identity` section (DESIGN-core §8) declares the AWS-auth provider
(irsa/iamRolesAnywhere/tes/static) as a default; per-subsystem `region`/`endpointUrl` remain overrides.
`keyProvider.type` gains `env`.

**Disconnect tolerance (central).** Vault reads serve entirely offline. `SyncEngine` already treats a fetch
failure as non-fatal (keeps cache, increments a counter) — exactly the required behavior. **The KUBERNETES
default KeyProvider is therefore offline-capable (`env`/`file`), not `kms`-only** (FR-CRED-6): a `kms`-only
KEK needs connectivity *at unlock*, so a pod cold-booting **during a disconnect** would fail closed and
crash — violating NFR-DISCONNECT-1 (risk R10). `kms` via IRSA is opt-in and **must** pair with a
configurable offline fallback, because the code picks exactly one provider with no automatic fallback
(`Credentials.java:79-129`) — so the fallback has to be explicit.

**Parity.** Normative on-disk format + vectors shared across all four (Java canonical). Rust gates via
`credentials`/`credentials-aws`/`credentials-pkcs11` features and zeroizes DEKs; Java/TS don't zeroize
(platform limitation). The `env` KeyProvider + mountedDir source land in all four.

**Risks.** Trust boundary is the whole pod (namespacing is logical, not a security boundary); DEK in a
non-zeroized `byte[]` (Java); **shared-volume vault on RWX would race the host-local advisory lock — do not
co-write across pods** (per-pod vault or single sync-owner, FR-CRED-5); inline `pin` for pkcs11 is a leak
risk (prefer `pinEnv`); `endpointOverride` must not leak into prod manifests.

---

## 7. Parameters — `mountedDir` on ConfigMap/Secret; SSM via chain

**Seam today.** `gg.parameters()` is offline-first: exactly one **source** (`env` | `mountedDir` | `awsSsm`)
behind a cache; `get*` reads **only the cache** (never the network); a background daemon re-pulls declared
names/paths every `refreshIntervalSecs`; a failed refresh is logged/counted and **retains cached values**.
Remote sources (SSM) default to a **persistent encrypted cache reusing the credentials vault**; local
sources use an in-memory cache. `AwsSsmSource` builds its client via the SDK **default chain**.

**Kubernetes addition (largely already designed for k8s).**
1. **`mountedDir` is the first-class k8s path** and needs **no cluster RBAC** — the kubelet mounts a
   ConfigMap/Secret volume; the component reads files. It already skips the `..data` dotfile farm and
   supports `securePaths` to mark Secret subpaths as `secure` (cache-encrypted/redacted). Default on
   KUBERNETES.
2. **SSM-from-pod via the chain** — works under IRSA / IAM Roles Anywhere with no code change, only IAM
   trust + `region`/`endpointUrl`.
3. **ESO interplay** — ESO syncs SSM/Secrets-Manager into native Secrets/ConfigMaps that `mountedDir` then
   reads (operator holds AWS creds instead of the pod). Both `mountedDir`+ESO (idiomatic) and `awsSsm`+IRSA
   (self-contained) are valid.

**Config / schema.** No new section needed; `source.type` selects; `securePaths` lists secret subpaths;
`cache.path`/`cache.persist`/`cache.keyProvider` already exist.

**Disconnect tolerance.** Offline-first by construction: reads come from the cache; refresh failures keep
last-known values. **Caveat (FR-PARAM-3):** the persistent encrypted cache only survives pod restart if
`cache.path` is on a **PersistentVolume**; on ephemeral pod storage the offline guarantee is lost and a cold
start re-pulls (requires the source up). KEK custodian must itself be offline-capable for disconnected cold
boots.

**Parity.** Four-way, near line-for-line. Rust gates SSM behind `parameters-aws`; Java/Python gate at runtime
(optional jar / lazy boto3). No cross-source layering today (one source per component) — flag if a design
assumes "ConfigMap with SSM fallback".

**Risks.** No mtime short-circuit (full tree re-read each refresh tick); a Secret not listed in `securePaths`
is cached/surfaced unredacted (in-memory only, but unredacted); persistent-cache durability hinges on a PV;
unconditional `cache.put` each cycle (minor churn). Phase-2 audit/metrics not built.

---

## 8. Streaming — durable buffer on a StatefulSet + PVC

**Seam today.** One Rust core (`ggstreamlog`) drives all four languages over a C ABI; hosts only
`append` + read `stats`. A per-stream **durable on-disk buffer** (`buffer.path`: a directory of append-only
`.seg` segments + an atomic checkpoint; crash-recovers a torn tail on open) store-and-forwards to
**Kinesis/Kafka** with at-least-once delivery (commit-after-ack), exponential backoff, and `maxRetries=-1`
(retry forever — the disconnected case) by default. `buffer.type` is `disk` (durable) or `memory`
(non-durable ring). Sink AWS auth uses the SDK **default chain** (`kinesis.rs:33,59`). `StreamService.close()`
flushes + stops engines.

**Kubernetes addition (deployment topology + auth, not core changes).**
1. **PVC-backed buffer + StatefulSet (FR-STREAM-1/2).** For a `disk` buffer carrying must-not-lose
   telemetry, `buffer.path` **must** sit on a PersistentVolume, and the workload **must** be a StatefulSet
   with a per-pod `volumeClaimTemplate` (or single-replica Deployment + static PVC + `Recreate`). The buffer
   is **single-writer** per directory (one `BufWriter`); two pods on a shared RWX path would corrupt the
   segment log — use `ReadWriteOncePod` (k8s ≥1.29) where available, never shared-writer RWX. Path templating
   (`{ComponentName}`/`{ThingName}`) gives a per-identity subdir under the mount.
2. **CSI-agnostic (FR-STREAM-3).** Reference `StorageClass`/PVC abstraction, not cloud-specific volumes.
   On-prem drivers: local-path, Longhorn, Ceph/Rook, vSphere CSI, NFS. (EBS/EFS are the EKS-in-cloud case
   only; if used, EBS RWO is AZ-pinned — run ≥2 nodes per AZ to avoid reschedule deadlock; EFS is regional/
   RWX but keep a single writer.) Set `maxDiskBytes ≤ PVC capacity`.
3. **Sink auth via the chain (FR-STREAM-5).** Kinesis/Kafka auth via IRSA / IAM Roles Anywhere, no explicit
   creds; `endpoint_url`/`region` overridable for private endpoints.

**Config / schema.** Streaming config lives in the shared Rust core (`BufferConfig`/`BatchConfig`/
`DeliveryConfig`/`SinkConfig`); no schema change required for k8s beyond documenting the lossless recipe.
`buffer.type`/`buffer.path`/retention are config; **`StatefulSet`/PVC/`storageClassName` are chart-only
deployment concerns**, not config-schema fields (DESIGN-packaging §8). Note: **Java does not
template-resolve** the streaming config (`StreamService.java:46` — caller must pre-resolve), while the Rust
façade does — a parity gap to close.

**Disconnect tolerance (this subsystem IS the store-and-forward engine).** This is the canonical
intermittent-cloud case: buffer locally, drain when the link returns. **Lossless recipe (FR-STREAM-4):**
PVC + `onFull: block` (or generous `maxDiskBytes`) + `maxRetries: -1` + `fsync: always|perBatch`. Loss
sources to avoid: `emptyDir` (lost on reschedule), `memory` buffer (lost on restart), `onFull: dropOldest`
overflow while disconnected, `maxAgeSecs` expiry, finite `maxRetries` poison-pill drop, and the
`fsync: interval` crash window. At-least-once means a crash between ack and commit re-sends (duplicates,
not loss — dedup downstream).

**Back-pressure caveat (FR-STREAM-8 / risk R11).** `onFull: block` guarantees no loss but **stalls the
producing thread when a full buffer meets a lengthy disconnect** — size `maxDiskBytes` for the worst-case
disconnect window, keep `/livez` **decoupled from a blocked `append`** (a stall must not trigger a
liveness-restart storm), and choose `dropOldest` (counted drops) where staying live matters more than
completeness. This block-vs-dropOldest call is the central durability/availability tradeoff for an
intermittent fleet. **Shutdown flushes to *disk*, not to the cloud:** on SIGTERM the buffer fsyncs and
persists on its PVC and resumes draining on restart, so `terminationGracePeriodSeconds` only needs to
cover unsubscribe + fsync, **not** backlog export (FR-STREAM-6).

**Parity.** Engine behavior is uniform by construction (one core, four thin façades). Divergence risk is in
the façade: template resolution (Java gap above), builders, stats mapping. `IStreamService` DI seam exists
in Rust/TS; Java/Python wrap the concrete native service.

**Risks (R3).** Buffer on `emptyDir` silently loses telemetry on every pod move — no code guard, only
topology + docs. Single-writer unenforced across processes (prevent via StatefulSet + RWOP). Kafka SASL/TLS
rotation gap (static librdkafka properties; the live-refresh credential provider is designed-not-built);
multi-arch native artifact matrix; one export thread + private tokio runtime per stream multiplies threads
on constrained edge pods.

---

## 9. Subsystem × concern matrix (quick reference)

| Subsystem | New k8s surface | New inbound port? | Cloud dependency | Disconnect behavior |
|---|---|---|---|---|
| config | `CONFIGMAP` source | no | none (local mount) | reject-and-keep; reload offline |
| messaging | Service DNS + ConfigMap/Secret config | no | IoT Core (optional half) | local broker continues; reconnect IoT Core |
| metrics | `prometheus` `/metrics` | **yes** (`/metrics`) | CloudWatch/AMP optional | pull = gaps only; push optional |
| heartbeat | `/livez` `/readyz` `/startupz` + SIGTERM | **yes** (health) | none | livez excludes broker; keeps sampling |
| logging | stdout-JSON sink | no | log agent (cluster) | stdout always works |
| credentials | `env` KeyProvider; optional CSI/ESO source | no | KMS/SM (unlock/sync) | vault reads offline; sync non-fatal |
| parameters | `mountedDir` default | no | SSM optional | cache reads offline; refresh non-fatal |
| streaming | PVC buffer + StatefulSet | no | Kinesis/Kafka sink | store-and-forward; drain on reconnect |

Three subsystems open **inbound** listeners that did not exist before (metrics `/metrics`, heartbeat
health) — declared as container ports, opt-out, restricted-bindable (NFR-SEC-4). Everything else is
outbound or local, and every cloud dependency is an *optional half* that degrades gracefully.
