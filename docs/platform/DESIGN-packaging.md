# Design — Kubernetes packaging & operations

> Companion to [DESIGN-core.md](DESIGN-core.md) / [DESIGN-subsystems.md](DESIGN-subsystems.md).
> **Status: PROPOSED.** How a edgecommons component is *deployed* on edge/on-prem Kubernetes: the Helm
> chart shape, workload choice, ConfigMap/Secret mounts, liveness/readiness/startup probes + graceful
> shutdown, Prometheus `ServiceMonitor`, RBAC/ServiceAccount, PVC (CSI-agnostic), edge AWS identity
> (IRSA / IAM Roles Anywhere — **not** EKS Pod Identity), NetworkPolicy, and sidecar/init patterns.
> All examples are illustrative (no code shipped in this phase).

---

## 1. Principle: compose, don't reinvent

The chart **renders the component workload and composes existing operators** rather than building new
controllers (FR-PKG-2). The division of labor:

| Concern | Owner |
|---|---|
| Component config | edgecommons `CONFIGMAP` source ← Helm-rendered `ConfigMap` |
| Cloud-secret sync (optional) | **External Secrets Operator (ESO)** or **Secrets Store CSI Driver** → mounted Secret/files |
| Scrape config | **Prometheus Operator** ← Helm-rendered `ServiceMonitor` |
| Restart-on-change (for `subPath`/env/immutable config) | **Stakater Reloader** (annotation) |
| Everything else (workload, probes, RBAC, PVC, NetworkPolicy) | the edgecommons Helm chart |

The chart **MUST degrade gracefully** when an operator is absent: no Prometheus Operator → emit a
`Service` only (and document manual scrape); no ESO/CSI → use a plain `Secret`/the vault's own sync.

## 2. Workload choice

| Component shape | Workload | Why |
|---|---|---|
| Stateless (no durable streaming buffer, or `memory` buffer) | **Deployment** | interchangeable pods; HPA-friendly |
| Durable streaming `disk` buffer that must survive reschedule | **StatefulSet** + `volumeClaimTemplates` | stable per-pod PVC; single-writer safe (DESIGN-subsystems §8) |
| Single durable instance, simple | single-replica **Deployment** + static PVC + `strategy: Recreate` | avoids two pods briefly holding an RWO volume during a roll |

The chart selects via `values.workload.kind` (`Deployment`|`StatefulSet`).

## 3. Config & identity mounts

```yaml
volumes:
  - name: edgecommons-config                      # component config (CONFIGMAP source)
    configMap: { name: {{ .Release.Name }}-config }
  - name: edgecommons-certs                        # MQTT mTLS material (IoT Core / broker)
    secret: { secretName: {{ .Values.messaging.tlsSecret }} }
  - name: podinfo                                # Downward API VOLUME — metadata.* ONLY (not spec.*/status.*)
    downwardAPI:
      items:
        - { path: "namespace",  fieldRef: { fieldPath: metadata.namespace } }
        - { path: "podname",    fieldRef: { fieldPath: metadata.name } }
        - { path: "thing-name", fieldRef: { fieldPath: metadata.annotations['edgecommons.io/thing-name'] } }
volumeMounts:
  - { name: edgecommons-config, mountPath: /etc/edgecommons/config }   # WHOLE volume, never subPath (reload!)
  - { name: edgecommons-certs,  mountPath: /etc/edgecommons/certs, readOnly: true }
  - { name: podinfo,          mountPath: /etc/podinfo, readOnly: true }
env:                                              # spec.*/status.* fields are env-ONLY (not volume-able)
  - { name: NODE_NAME, valueFrom: { fieldRef: { fieldPath: spec.nodeName } } }
  - { name: POD_IP,    valueFrom: { fieldRef: { fieldPath: status.podIP } } }
args: ["--platform", "kubernetes", "-c", "CONFIGMAP", "/etc/edgecommons/config"]
```

- **Never `subPath`** for the config mount — it breaks hot-reload silently (DESIGN-subsystems §1). The
  chart template MUST mount the whole volume; if a user forces `subPath`, the chart adds the Reloader
  annotation (`reloader.stakater.com/auto: "true"`) and documents that reload is restart-based.
- **Annotations/labels are exposable only via the downwardAPI volume** (its items use `fieldRef` for
  `metadata.*`), so identity from `edgecommons.io/thing-name` uses the volume; conversely **`spec.nodeName`/
  `status.podIP` are env-only** (`valueFrom.fieldRef`) and are **not** volume-able — hence the split above
  (DESIGN-core §6.2).

## 4. Probes & graceful shutdown

Recommended manifest block (from the health/lifecycle research; FR-HB-1/2/3):

```yaml
ports:
  - { name: health,  containerPort: 8081 }
  - { name: metrics, containerPort: 9090 }       # /metrics for the prometheus target
startupProbe:   { httpGet: { path: /startupz, port: health }, periodSeconds: 5,  failureThreshold: 30 }   # 150s budget
livenessProbe:  { httpGet: { path: /livez,    port: health }, periodSeconds: 10, failureThreshold: 3, timeoutSeconds: 2 }
readinessProbe: { httpGet: { path: /readyz,   port: health }, periodSeconds: 5,  failureThreshold: 2 }
lifecycle:
  preStop: { sleep: { seconds: 5 } }             # native sleep (k8s ≥1.30); lets EndpointSlice removal propagate
terminationGracePeriodSeconds: 30                # ≥ preStop + measured unsubscribe/flush time
```

Key points:
- **`/livez` must not check the broker** (a broker/cloud outage must not cause restart storms);
  **`/readyz` gates traffic** on messaging-connected + subscriptions-confirmed and flips to 503 the
  instant SIGTERM arrives.
- The grace period covers **preStop + post-SIGTERM drain together**. Size
  `terminationGracePeriodSeconds` to comfortably exceed preStop-sleep + the time to unsubscribe all
  topics and flush streaming buffers; raise it if streaming flush is slow. If pods get SIGKILLed, the
  unsubscribe never runs and the subscription leak (reasonCode 151) returns.
- The component itself must wire SIGTERM → `shutdown()` (DESIGN-subsystems §4); the manifest only
  provides the budget and drain delay.
- **Streaming shutdown flushes to *disk*, not to the cloud.** On SIGTERM the streaming buffer fsyncs and
  persists on its PVC and resumes draining to Kinesis/Kafka on restart — graceful shutdown does **not**
  attempt to drain the backlog to the cloud. So `terminationGracePeriodSeconds` only needs to cover
  unsubscribe + fsync, **not** backlog export (FR-STREAM-6); a large backlog on a slow link does not need
  a large grace period.
- **Endpoint removal is deletion-driven**, not readiness-driven: the EndpointSlice flips to not-ready when
  the pod is marked Terminating, and `preStop` runs *before* SIGTERM — so `/readyz`→503 (which fires once
  SIGTERM lands, post-`preStop`) is belt-and-suspenders, not the primary drain mechanism.

## 5. Metrics scrape — ServiceMonitor

```yaml
# rendered when values.metrics.serviceMonitor.enabled and the Prometheus Operator is present
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata: { name: {{ .Release.Name }}, labels: { release: kube-prometheus-stack } }
spec:
  selector: { matchLabels: { app: {{ .Release.Name }} } }
  endpoints: [ { port: metrics, path: /metrics, interval: 30s } ]
```

PodMonitor is the variant for Job/CronJob/sidecar pods with no Service. Absent the Prometheus Operator,
the chart emits only the `Service` exposing the `metrics` port and documents manual scrape config. On
edge clusters this is the **default, fully offline-capable** observability path; CloudWatch/AMP via ADOT
or EMF-over-stdout is an optional add-on when cloud connectivity exists.

## 6. RBAC & ServiceAccount (least privilege)

The default edgecommons component needs **no Kubernetes API permissions**: config and secrets arrive as
*mounted volumes* (the kubelet does the fetch), metrics are *scraped* (Prometheus reads, the pod does
not call the API), and identity comes from the Downward API. So the default `Role` is **empty**
(NFR-SEC-2).

```yaml
serviceAccount:
  create: true
  name: {{ .Release.Name }}
  annotations: {}            # IRSA role-arn goes here when used (§7)
# No Role/RoleBinding by default. RBAC is added ONLY if the optional API-watch config source is enabled
# (get/watch on the specific configmaps/secrets), or by the composed operators (ESO/Prometheus) for
# their OWN controllers — never granted to the component pod.
```

Cluster-admin is never used; any RBAC is namespaced and scoped to named resources.

## 7. Edge AWS identity (IRSA / IAM Roles Anywhere — not Pod Identity)

edgecommons constructs all AWS clients via the SDK **default credential provider chain** with **no explicit
credentials** (DESIGN-subsystems §6/§7/§8), so identity is wired entirely in the manifest. For the
edge/on-prem target, in priority order (FR-PKG-3):

**(a) IRSA via OIDC federation** — works on EKSA and self-managed clusters whose API-server OIDC issuer
is reachable by AWS. The chart annotates the ServiceAccount; the cluster's mutating webhook injects the
projected token + `AWS_ROLE_ARN`/`AWS_WEB_IDENTITY_TOKEN_FILE`; the SDK's web-identity provider does the
STS exchange:

```yaml
serviceAccount:
  annotations:
    eks.amazonaws.com/role-arn: arn:aws:iam::<acct>:role/<edgecommons-component-role>
# Self-managed: register the API server's OIDC issuer as an IAM OIDC provider; the projected-token flow
# is identical ("hand-rolled IRSA").
```

**(b) IAM Roles Anywhere (X.509)** — the primary path for **private/air-gapped-leaning** clusters with no
public API-server exposure. The chart mounts the client cert/key (from a Secret) and the credential
helper config; the SDK process-credential provider exchanges the cert for short-lived STS creds. This
aligns with edgecommons' existing IoT X.509 heritage.

**(c) Static keys in a Secret** — last resort (dev / fully air-gapped that still needs occasional AWS).
Mounted as env or file; least secure; manual rotation; documented as non-default.

**EKS Pod Identity is NOT supported as a path** — it is EKS-cloud-only and the target is edge/on-prem. It
is mentioned only as an incidental convenience for someone who happens to run on EKS-in-cloud.

> Because the SDK chain stops at the first match, a stray static key shadows IRSA/IAM-Roles-Anywhere —
> the chart must not set AWS env keys unless `identity.provider: static` is explicitly chosen
> (NFR-SEC-1).

## 8. Persistent storage for streaming (CSI-agnostic)

```yaml
# StatefulSet volumeClaimTemplate — for a durable `disk` streaming buffer
volumeClaimTemplates:
  - metadata: { name: edgecommons-buffer }
    spec:
      accessModes: ["ReadWriteOncePod"]           # single-writer guarantee (k8s ≥1.29); else ReadWriteOnce
      storageClassName: {{ .Values.streaming.storageClassName }}   # local-path | longhorn | ceph-rbd | vsphere | nfs | (ebs/efs on EKS)
      resources: { requests: { storage: {{ .Values.streaming.size | default "5Gi" }} } }
volumeMounts:
  - { name: edgecommons-buffer, mountPath: /var/lib/edgecommons/streams }
```

Guidance (DESIGN-subsystems §8): `storageClassName` is **operator-chosen**, never assumed to be
EBS/EFS. Set `buffer.maxDiskBytes ≤` the PVC `storage` request. Reclaim policy should avoid deleting the
backing volume. `emptyDir` is acceptable **only** for the `memory` buffer or explicitly loss-tolerant
`disk` streams. On EKS-in-cloud only: EBS RWO is AZ-pinned (run ≥2 nodes per buffer AZ); EFS is regional
but keep a single writer.

## 9. NetworkPolicy (optional egress hardening)

```yaml
# rendered when values.networkPolicy.enabled; needs a policy-capable CNI (Calico/Cilium/VPC-CNI)
egress:
  - to: [ { namespaceSelector: {...}, podSelector: { matchLabels: { app: emqx } } } ]   # in-cluster broker
    ports: [ { protocol: TCP, port: 1883 }, { protocol: TCP, port: 8883 } ]
  - to: [ ... ]                                                                          # IoT Core endpoint
    ports: [ { protocol: TCP, port: 8883 }, { protocol: TCP, port: 443 } ]
  - ports: [ { protocol: UDP, port: 53 }, { protocol: TCP, port: 53 } ]                  # DNS (required)
  # + egress to AWS STS/KMS/Secrets Manager/SSM/Kinesis (443) for any ENABLED cloud subsystem —
  #   omitting these silently breaks IRSA/STS and all cloud cooperation. These are dynamic IPs.
```

FQDN-based egress (Calico/Cilium) is **required** for the IoT Core endpoint **and** the AWS API endpoints
(STS/KMS/Secrets Manager/SSM/Kinesis), which are dynamic IPs — a CIDR-only policy that forgets them
silently breaks IRSA and cloud cooperation (FR-PKG-4 / risk R-egress). Default off; opt-in hardening.

## 10. Sidecar / init-container patterns

- **In-cluster MQTT broker** — deploy EMQX via its Kubernetes Operator (StatefulSet + headless Service for
  identity; ClusterIP Service for client DNS; TLS from a `kubernetes.io/tls` Secret). Components point
  `local.host` at the broker's Service DNS. This is the in-cluster realization of today's test-infra
  `compose.yaml` EMQX.
- **Cert provisioning (init)** — an init-container or cert-manager populates the mTLS Secret before the
  component starts; the component reads `certPath/keyPath/caPath` from the mount unchanged.
- **Reloader (sidecar-less)** — annotate the workload for restart-on-change when config can't hot-reload
  in-process (forced `subPath`, env config, immutable ConfigMaps).
- **ADOT Collector (optional)** — scrapes `/metrics` and remote-writes to AMP, or receives EMF; keeps
  cloud egress centralized and out of the component. Optional, connectivity-gated.

## 11. Images

- **Multi-arch** (`linux/amd64` + `linux/arm64`) so the bundled `edgestreamlog` native artifact matches the
  node arch (DESIGN-subsystems §8 / FR-STREAM-7) — edge fleets are frequently arm64.
- Java images run with `--enable-native-access=ALL-UNNAMED` (Panama/FFM requirement for streaming).
- Prefer a read-only root filesystem + non-root `securityContext`; the stdout-JSON logging default means
  no writable log path is required, and only the streaming buffer mount + tmpfs secrets need to be writable.
- **Namespace is a *soft* boundary.** Any co-tenant pod / the ServiceAccount can reach mounted secrets, and
  KMS-at-rest does not protect against an in-namespace compromise (the API server decrypts on read). Deploy
  the component in a **dedicated namespace** with a **restricted PodSecurity standard** (NFR-SEC-2/3).

## 12. Values surface (illustrative)

```yaml
platform: kubernetes
workload: { kind: StatefulSet, replicas: 1 }
image: { repository: ..., tag: ..., pullPolicy: IfNotPresent }
config:    { configMapName: "" }                  # rendered edgecommons config
messaging: { transport: dualMqtt, localBrokerDNS: emqx.mqtt.svc.cluster.local, iotCoreEndpoint: "", tlsSecret: "" }
metrics:   { target: prometheus, port: 9090, serviceMonitor: { enabled: true } }
health:    { enabled: true, port: 8081 }
identity:  { provider: irsa, roleArn: "" }         # irsa | iamRolesAnywhere | static | none
streaming: { enabled: false, storageClassName: "", size: 5Gi }
externalSecrets: { enabled: false }                # compose ESO
networkPolicy:   { enabled: false }
terminationGracePeriodSeconds: 30
```

Defaults reflect the KUBERNETES profile (DESIGN-core §3) and the edge-with-intermittent-cloud connectivity
model: dual-MQTT, Prometheus pull, health endpoint on, offline-capable everything; cloud cooperation is
expected but every cloud path tolerates lengthy disconnects (AWS reachability is never required to keep
running). `credentials`/`parameters` are driven by the component **config document** (mounted via the
ConfigMap), not chart values — only their deployment-affecting bits (a PVC for a persistent parameter
cache, the IRSA `identity` binding) surface here.

## 13. Distribution & publishing (libraries + component images)

**The problem.** A scaffolded component is built into a container image in a **clean build context** that has
none of the monorepo. But the templates depend on the library by a **local path** (`Rust` `edgecommons = { path
= "<<EDGECOMMONS_PATH>>" }`, `TS` `"file:<<EDGECOMMONS_PATH>>"`) or an **unpublished** coordinate (`Python`
`edgecommons`, `Java` `com.mbreissi.edgecommons:edgecommons` from local `~/.m2`). So a scaffold cannot build an
image — and therefore cannot be deployed to Kubernetes — until each library is obtainable from a registry the
build can reach. (The `test-infra/k8s` image works only because it builds from the *monorepo root* and copies
`libs/` in; that is the example, not an arbitrary scaffold.)

**Decision — GitHub-native private distribution** (the repo `mbreissi/edgecommons` is private, so all of these are
private by default; resolved 2026-06-26):

| Artifact | Private channel (now) | Coordinate / ref | Public later (low-cost swap) |
|----------|-----------------------|------------------|------------------------------|
| Component **images** | **ghcr.io** | `ghcr.io/edgecommons/<component>` | same |
| **Java** lib | GitHub Packages **Maven** | `com.mbreissi.edgecommons:edgecommons` | Maven Central (same coord; add GPG signing) |
| **TypeScript** lib | GitHub Packages **npm** | **`@edgecommons/edgecommons`** (renamed from `@breissinger`) | public npm under `@mbreissi` |
| **Python** lib | `pip git+https` | `git+https://github.com/edgecommons/edgecommons.git#subdirectory=libs/python` | PyPI `edgecommons==x.y` |
| **Rust** lib | cargo **git dep** | `edgecommons = { git = "https://github.com/edgecommons/edgecommons", tag = "rust-lib/vX.Y.Z" }` | crates.io `edgecommons = "x.y"` (⚠ verify name free before first public release) |

GitHub Packages has **no native PyPI/crates registry**, hence the git-based deps for Python/Rust; both resolve
against the private repo with the built-in `GITHUB_TOKEN`. Going public is a registry URL/credential change in
`release.yml` plus a one-line consumer dependency swap — *provided names/coordinates stay stable*, so they are
locked now (the npm scope `@mbreissi` is the only consumer-visible rename, done once, up front).

**Dual-mode scaffold Dockerfile.** Each template ships a `Dockerfile` with a build arg
`EDGECOMMONS_SOURCE = registry | local`:
- `registry` (default) — resolves the library from the channel above (needs `GITHUB_TOKEN` build secret). The
  path real users take.
- `local` — vendors the library from a monorepo checkout (the `test-infra` approach). Used for in-repo dev, CI,
  and **offline validation** (build the image, `kind load`, deploy — no registry/credentials needed). This is
  how the deploy QuickStart is validated without consuming Actions or publishing.

Multi-arch + non-root + read-only-root carry over from §11.

**Per-scaffold k8s manifests.** Each template ships `k8s/` (raw, `kubectl apply`-able): `configmap.yaml`
(component config incl. the `messaging` section, mounted as a *directory* at `/etc/edgecommons`) + `deployment.yaml`
(image ref, Downward-API identity env, httpGet probes on `:8081`, ConfigMap volume, `/tmp` emptyDir, no args —
`--platform auto` detects KUBERNETES from the SA token). The shared Helm chart (`test-infra/k8s/chart`) remains
the richer path; these raw manifests are the minimal hello-world deploy.

**`release.yml` wiring** (extends the existing skeleton; all publish steps **gated on secrets + manual
`workflow_dispatch`**, never auto-publish on a plain push): per-tag-prefix build+publish — Java→GH Packages
Maven, TS→GH Packages npm (`@mbreissi`), Python→git-tag (consumers `pip git+https@<tag>`), Rust→git-tag
(consumers `cargo { git, tag }`), and a component-image job pushing multi-arch images to `ghcr.io`.

**Sequencing & cost.** Decision (2026-06-26): **hold all CI** — do every file/wiring change locally, validate
via the Dockerfile `local` mode + `kind`, and run `release.yml` only on an explicit later go (conserves the
~Actions budget). The interactive-CLI follow-on (a `prompts` + conditional-files extension to
`edgecommons-template.json`) **has since been built**: `create-component`'s `--platforms`/`-i` wizard and
`--dep-source` flag (`cli/edgecommons_cli/commands/create_component.py`) gate manifest `conditional` blocks
so the `Dockerfile`/`k8s/` artifacts emit **only when the user targets Kubernetes** and the dependency
source (registry vs local) is chosen at scaffold time (e.g. `templates/rust/edgecommons-template.json`'s
`conditional` entry on `platform:KUBERNETES`).
