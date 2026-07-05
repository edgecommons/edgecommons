# Platform model (platform × transport) for ggcommons — Executive Summary & index

> **Status: Phase 1 SHIPPED on `main` (v0.2.0); Phase 2+ still design-for-review.** Phase 1 (the
> KUBERNETES platform profile, the `platform × transport` axes, the precedence resolver + auto-detect,
> ConfigMap/Downward-API config, HTTP health, Prometheus metrics, stdout-JSON logging, and the env-KEK
> vault) is live in all four languages; the additive Phase 2+ profiles (ECS/Nomad/systemd) and the
> operator remain design-for-review. This directory is a requirements + design
> package for **re-architecting ggcommons' runtime model into two orthogonal axes — `platform ×
> transport`** — re-expressing today's GREENGRASS (edge/Nucleus) and STANDALONE (bare host/Docker) as
> platform *profiles*, and adding **Kubernetes** as the first new platform (which the model enables and
> includes, but is not limited to — future targets like ECS/Nomad/systemd are additive profiles). The
> work keeps the same libraries, the same four-language parity, and (with deliberate, additive
> exceptions) the same on-wire and on-disk contracts.
>
> Author target: **edge / on-prem Kubernetes** — **EKS Anywhere (EKSA)** and generic self-managed
> CNCF clusters (k3s, RKE2, OpenShift on-prem, kubeadm). **EKS-in-cloud is a non-goal** (it works
> incidentally; we do not optimize for it).
>
> **Connectivity model: edge-first with intermittent cloud cooperation.** Components run at the edge
> and *do* use cloud services (IoT Core, Secrets Manager, SSM, Kinesis, CloudWatch) where appropriate —
> but the link is intermittent, so every cloud-dependent path must tolerate **lengthy disconnects**
> (offline-first, store-and-forward, reconnect-and-resume) and degrade gracefully, never crashing the
> component. Fully air-gapped is the *supported extreme* of that tolerance, not the default.

---

## 1. The problem

ggcommons today abstracts a component's cross-cutting concerns (config, messaging, metrics,
heartbeat, logging, credentials, parameters, streaming) behind one API so component authors write
only business logic. That abstraction is selected by a single CLI flag, `-m/--mode`, whose value is
one of two:

- **GREENGRASS** — Nucleus IPC transport + Greengrass deployment config (`GG_CONFIG`) + AWS auth via the
  SDK default credential chain (which resolves TES on-device); the on-device, Nucleus-managed path.
- **STANDALONE** — dual-MQTT transport (local broker + AWS IoT Core) + a positional messaging-config
  file; the "everywhere else (Kubernetes/Docker/bare host)" path.

Kubernetes already *works* under STANDALONE — you can run a ggcommons component in a pod today. But
it works as a bare process that happens to live in a container: it does not use any of Kubernetes'
native facilities. There is no ConfigMap config source, no `/metrics` endpoint for Prometheus, no
liveness/readiness probe surface, no Secret/Downward-API integration, no graceful-shutdown wiring to
the kubelet's SIGTERM, and the durable streaming buffer has no notion of a PersistentVolume. The
result is a second-class citizen on the very platform most on-prem/edge fleets are consolidating
onto.

## 2. The core insight

The `-m` flag secretly conflates **three** independent concerns (the transport switch is
`MessagingClient.java:42-61`):

1. **transport** — how messaging works (IPC vs MQTT);
2. **default config provider** — which `-c` source is implied;
3. **deployment platform** — what host-native facilities exist (Nucleus vs k8s vs bare host).

Kubernetes is the case that pulls these apart: it is *MQTT transport* × *Kubernetes platform*. Seven
of the eight subsystems' natural defaults (config source, metrics target, logging sink, heartbeat
surface, credentials key custodian, parameters source, streaming buffer location) vary by **platform**
independently of transport. Only messaging-transport is coupled to platform — and only in one
direction: **IPC requires the Nucleus, so `IPC × non-Greengrass` is invalid.**

## 3. The proposal: a pure two-axis runtime model

Replace the fused `-m {GREENGRASS,STANDALONE}` enum with **two named axes** (a deliberate, pre-1.0
**breaking CLI change** — backward compatibility is explicitly out of scope):

```
--platform   GREENGRASS | HOST | KUBERNETES | auto      (default: auto; platform-primary)
--transport  IPC | MQTT                                  (default: derived from platform; validated)
```

A **platform** is a named table of per-subsystem **defaults** (a "profile"). A **resolver** applies
them under one precedence rule:

> **explicit flag  ▸  explicit config value  ▸  platform-profile default  ▸  library default**

So a platform default never overrides anything the operator set explicitly — that precedence rule,
**together with profiles that faithfully reproduce today's GREENGRASS/HOST defaults** (DESIGN-core §9),
is what keeps the change backward-compatible *in behavior* even though the CLI surface changes.
`--platform auto` detects the environment from definitive signals (Nucleus IPC env vars → GREENGRASS;
service-account token → KUBERNETES; else HOST), always overridable, always logging its decision.
`--transport` stays a real, independent axis (you *can* set it) but is rarely needed because it
defaults from the platform; the resolver **rejects invalid pairs** (notably `IPC × {HOST,KUBERNETES}`)
at startup.

This is not a Kubernetes bolt-on. It re-expresses **today's two modes as two of N profiles**
(`GREENGRASS`, `HOST`) and makes Kubernetes the third — so future targets (ECS, Nomad, systemd) are
additive profiles, not new branches threaded through four languages.

## 4. What changes per subsystem (one line each)

| Subsystem | Kubernetes-native addition | Default on `KUBERNETES` |
|-----------|----------------------------|--------------------------|
| **config** | a `CONFIGMAP` source (mounted-volume; reuses the `FILE` hot-reload seam) | `CONFIGMAP` |
| **messaging** | broker via k8s Service DNS; MQTT config from ConfigMap+Secret, not a positional path | dual-MQTT — in-cluster broker (local pub/sub, survives disconnects) + IoT Core (cloud cooperation) |
| **metrics** | a **pull-based** `prometheus` `/metrics` target (+ ServiceMonitor) | `prometheus` |
| **heartbeat** | an HTTP **health endpoint** feeding liveness/readiness/startup probes; SIGTERM→graceful shutdown | health endpoint on |
| **logging** | a **stdout-JSON** sink (no in-process rotation; cluster agent owns it) | stdout-JSON |
| **credentials** | keep the local vault (offline-first); AWS auth via the SDK default chain; optional ESO/CSI delegation | local vault + SDK-chain KeyProvider |
| **parameters** | ConfigMap/Secret mount maps onto the existing `mountedDir` source | `mountedDir` |
| **streaming** | durable buffer on a **StatefulSet + PVC** (CSI-agnostic); SDK-chain auth for the sink | PVC-backed buffer |

## 5. AWS identity on edge/on-prem (the one decision the target most affects)

ggcommons already constructs every AWS SDK client **without explicit credentials**, relying on the
**SDK default credential provider chain** (`KmsKeyProvider`, `AwsSecretsManagerSource`, `AwsSsmSource`,
`KinesisSink` — all verified). That single seam means workload identity is an *environment* concern,
**not ggcommons code** — zero per-provider branches in any language. For the edge/on-prem target:

- **Default: IRSA via OIDC federation** — works on EKSA and self-managed clusters whose API-server
  OIDC issuer is reachable by AWS.
- **Private / air-gapped: IAM Roles Anywhere (X.509)** — no public API-server exposure; aligns with
  ggcommons' existing IoT X.509 heritage. This is the primary path for disconnected fleets.
- **Last resort: static keys in a Secret** (dev / fully air-gapped).
- **EKS Pod Identity is a non-goal** (EKS-cloud-only). Documented as an incidental convenience, never
  a default.

Cloud services are expected participants, not exceptions — but because the link to them is
**intermittent**, every cloud-dependent path must tolerate **lengthy disconnects**: the local vault
serves reads offline, parameters retain last-known values, the streaming buffer stores-and-forwards
until the link returns, and the in-cluster MQTT broker keeps local pub/sub flowing while IoT Core is
unreachable. A cloud outage must degrade gracefully, never crash the component. Fully air-gapped
operation is the supported extreme of this same tolerance.

## 6. Packaging & operator

- **Ship a Helm chart** that renders the Deployment/StatefulSet, ConfigMap, probes, RBAC/ServiceAccount,
  PVC, and a `ServiceMonitor`; it **composes existing operators** — External Secrets Operator (for the
  optional cloud-secret-sync path) and the Prometheus Operator (for scrape config).
- **Do not build a custom operator now.** Nothing in ggcommons is a stateful Day-2 problem that
  needs encoded operational knowledge; an operator would also be the first Go binary in an otherwise
  Java/Python/Rust/TS repo. A `GgcommonsComponent` CRD **sketch** is included for completeness, with
  the explicit triggers that would justify revisiting it (see [DESIGN-operator.md](DESIGN-operator.md)).

## 7. Invariants (held) and breakage (accepted)

**Accepted breakage (pre-1.0):** the `-m` CLI contract is removed (no alias shim); legacy invocations
fail fast. The programmatic builder API is free to evolve (it gains platform/transport).

**Invariants that MUST hold:**
- **Greengrass runtime behavior is preserved** — the CLI to launch it changes; what it *does* must not
  regress. The existing test suites are the oracle (tests that invoke via `-m` are rewritten to the
  new flags). This is the safety property of Phase 0.
- The **wire envelope**, **config-schema semantics** (additive sections only), **vault conformance
  vectors**, and **cross-language interop** are preserved.

## 8. Phasing

- **Phase 0 — behavior-preserving refactor (full detail in [DESIGN-core.md](DESIGN-core.md)).**
  Introduce the `Platform`/profile abstraction + precedence resolver; re-express GREENGRASS and HOST
  as profiles; replace the mode switch with a transport/profile resolution; rewrite tests to the new
  CLI. **Success criterion: existing behavior is unchanged** (suites green; Greengrass on-device
  behavior identical).
- **Phase 1 — the KUBERNETES profile (additive).** `CONFIGMAP` source, `prometheus` target,
  stdout-JSON sink, health endpoint + SIGTERM wiring, Downward-API identity, PVC-aware streaming,
  ConfigMap/Secret-sourced messaging, `--platform auto` enabled by default (the detector itself is built
  in Phase 0), the Helm chart.
- **Phase 2+ — future platforms** (ECS/Nomad/systemd) as additive profiles.

## 9. Top risks (full register in [REQUIREMENTS.md](REQUIREMENTS.md) §NFR and each design doc)

1. **Four-language rearchitecture of the most load-bearing code** (the init/CLI path). Mitigated by
   Phase 0's "behavior unchanged" oracle and Java-canonical-first sequencing.
2. **Config hot-reload on ConfigMap volumes** — the kubelet's atomic `..data` symlink swap and the
   **`subPath`-never-updates** gotcha. The file watcher must watch the *directory* and re-arm on
   delete; verify in all four languages.
3. **Streaming telemetry loss** if the durable buffer lands on `emptyDir`/ephemeral storage — needs a
   StatefulSet + PVC and a single-writer guarantee.
4. **The `prometheus` target inverts the push lifecycle** — `flush()`/`emitMetricNow()` become no-ops;
   any caller relying on flush-before-exit gets nothing until the next scrape. Needs explicit docs.
5. **Init-order circularity** — transport is chosen before component config loads, so a config-derived
   `transport`/`platform` override cannot be honored from the component config section; the resolver
   must run on parse-time inputs (flags/env/detection) only.

## 10. Document index

| Doc | Contents |
|-----|----------|
| [REQUIREMENTS.md](REQUIREMENTS.md) | Functional (FR-*) and non-functional (NFR-*) requirements with acceptance criteria, by area. |
| [DESIGN-core.md](DESIGN-core.md) | The two-axis runtime model, platform profiles, the precedence resolver, auto-detection, the new CLI contract, identity resolution, init order, schema additions, and the **full Phase-0** behavior-preserving migration. |
| [DESIGN-subsystems.md](DESIGN-subsystems.md) | Per-subsystem design for all eight, the platform-defaults matrix, new sources/targets/sinks, and config schema deltas. |
| [DESIGN-packaging.md](DESIGN-packaging.md) | Helm chart shape, liveness/readiness/startup probes, graceful shutdown, ServiceMonitor/Prometheus, RBAC/ServiceAccount, PVC, sidecar/init patterns, and edge/on-prem AWS identity wiring. |
| [DESIGN-operator.md](DESIGN-operator.md) | The optional `GgcommonsComponent` CRD sketch and the explicit "don't build it yet / build it when…" recommendation. |
| [DESIGN-uns.md](DESIGN-uns.md) | The **Unified Namespace**: topic grammar, message classes, configurable hierarchy + top-level identity (distributed via `../SHARED_CONFIG.md`), the `messaging()`/`uns()`/facade API, streaming enrichment, and the `uns-bridge` site-bus realization. Concretizes [DESIGN-channels.md](DESIGN-channels.md); consumed by the `edgecommons/edge-console` component. |
| [UNS-CANONICAL-DESIGN.md](UNS-CANONICAL-DESIGN.md) | **Implementation companion to DESIGN-uns**: concrete Java-canonical API shapes (`MessageIdentity`, `Uns`, the instance handle, the reserved-class guard + privileged-publish seam, the `request()` deadline, MQTT LWT) with per-language mirror notes, plus the running **decisions register (D‑U1…D‑U24)** and the phased build checklist. Source of truth for the build. |
| [DESIGN-uns-bridge.md](DESIGN-uns-bridge.md) | **Phase-3 companion to DESIGN-uns (PROPOSED)**: the config-driven **named/secondary messaging connection** (M8 / D‑U17 → `messaging.connections` + `gg.messaging("site")`), the **`uns-bridge`** component design (six-class relay, `reply_to` rewrite with TTL'd correlation map, `tags._relay` hop-tag loop protection, per-class uplink policy + drop counters, site-broker LWT → whole-device UNREACHABLE), the **site-broker recipes** (M2: HOST Docker / GG container / K8s), dual-EMQX local testability, and the **Phase-3 decisions register (D‑B1…D‑B15)** + build slices. |
| [PARITY.md](PARITY.md) | Per-language implementation plan and four-way parity deltas (Java canonical; Python/Rust/TS specifics, Rust cargo features, service-interface seams). |

> Design claims are backed by inline `file:line` citations to the source tree (e.g.
> `libs/java/.../MessagingClient.java`) and source URLs in each design doc; the underlying subsystem
> code-maps and k8s/EKS research that grounded this package are available on request.
