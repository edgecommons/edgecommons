# Design — Operator / CRD (sketch + recommendation)

> Companion to [DESIGN-packaging.md](DESIGN-packaging.md). **Status: PROPOSED — sketch only.**
> Requirement FR-OP-1: provide a `GgcommonsComponent` CRD *sketch* and an explicit recommendation, but
> **do not build a production operator** in this effort.

---

## 1. Recommendation: don't build one now — ship a Helm chart that composes existing operators

A Kubernetes **Operator** is a custom controller that watches a Custom Resource (CRD) and runs a
reconcile loop driving actual state toward desired state. It earns its complexity only for **stateful
Day-2 operations** a one-shot installer can't express: backup/restore, failover, schema-aware upgrades,
continuous drift correction, dynamic regeneration. **Helm**, by contrast, renders manifests once and has
no runtime-state awareness — the industry default is "Helm for declaratively-complete apps, an operator
only when lifecycle intelligence is genuinely required."

**Nothing in ggcommons is a stateful Day-2 problem today.** Everything a `GgcommonsComponent` CRD would
"manage" already maps onto mature, off-the-shelf pieces:

| ggcommons concern | Already owned by |
|---|---|
| Render component config (+ `{ComponentName}`/`{ThingName}` substitution) into a ConfigMap | **Helm** templating |
| Central secret sync (Secrets Manager / SSM) + at-rest encryption | **External Secrets Operator** / **Secrets Store CSI Driver** |
| Scrape config for `/metrics` | **Prometheus Operator** (`ServiceMonitor`) |
| Deployment/StatefulSet, probes, HPA, PVC, RBAC, NetworkPolicy | **Helm chart** (DESIGN-packaging) |
| AWS identity | IRSA / IAM Roles Anywhere (manifest wiring) |

So a custom operator would add a CRD + a controller binary + RBAC + leader election for **purely
ergonomic** gain (one CR instead of ~5 correlated objects), not new capability — and it would be the
**first Go binary** in an otherwise Java/Python/Rust/TS monorepo, adding a runtime, toolchain, and
four-way-parity burden. **Recommendation: ship the Helm chart; compose ESO + Prometheus Operator; use
IRSA/IAM-Roles-Anywhere for identity. Defer the operator.**

## 2. When to revisit (the triggers)

Build a thin operator **only if** a concrete stateful need emerges that Helm + existing operators cannot
express. Watch for:

- **Dynamic stream lifecycle** — programmatically create/remove ggstreamlog streams not in static config
  (the deferred "dynamic streams" design) needs reconcile-style management of PVCs/StatefulSets.
- **Automated vault re-keying / rotation** — rotating the credentials vault KEK or re-wrapping DEKs
  across a fleet is a stateful, ordered, knowledge-bearing operation.
- **Config drift correction at admission** — validating/auto-correcting component config against
  `schema/ggcommons-config-schema.json` as an admission/reconcile step.
- **Orchestrated platform transitions** — e.g. coordinating GREENGRASS↔KUBERNETES migration of a fleet.

Absent one of these, the operator is gold-plating.

## 3. `GgcommonsComponent` CRD — sketch (for the day it's justified)

If built, the operator should **own exactly one CR** that aggregates the correlated objects and
**delegate** to ESO and the Prometheus Operator rather than duplicate them. Illustrative shape:

```yaml
apiVersion: ggcommons.io/v1alpha1
kind: GgcommonsComponent
metadata: { name: com.example.MyComponent }
spec:
  image: ghcr.io/example/mycomponent:1.2.3
  platform: kubernetes                 # resolver platform (DESIGN-core)
  transport: dualMqtt                  # ipc is invalid here; webhook validates the pair
  identity: { provider: irsa, roleArn: arn:aws:iam::...:role/... }
  config:                              # the ggcommons config-schema document (validated against the canonical schema)
    component: { name: com.example.MyComponent }
    logging: { ... }
    metricEmission: { target: prometheus }
    heartbeat: { ... }
    health: { enabled: true, port: 8081 }
  messaging: { localBrokerDNS: emqx.mqtt.svc.cluster.local, tlsSecretRef: my-mqtt-certs }
  secretRefs:                          # delegated to ESO → mounted Secret; `from` is provider-agnostic
    - { name: db-password, from: secretsManager, key: prod/mycomponent/db }   # AWS Secrets Manager, OR
    - { name: api-token,   from: vault,          key: kv/mycomponent/token }  # on-prem HashiCorp Vault, etc.
  streaming: { enabled: true, storageClassName: longhorn, size: 10Gi }   # → StatefulSet + PVC
  scaling: { replicas: 1 }             # StatefulSet for durable buffer
status:                                # observed; reconstructed each reconcile, never source of truth
  renderedConfigHash: "sha256:..."
  childResources: [ Deployment/..., ConfigMap/..., ExternalSecret/..., ServiceMonitor/... ]
  conditions: [ { type: Ready, status: "True" } ]
  observedGeneration: 7
```

Controller contract (kubebuilder/operator-sdk, Go): idempotent `Reconcile(ctx, req)`; handle `NotFound`
(deletion) gracefully; set `ownerReferences` on children for GC + watch-triggering; return
`ctrl.Result{RequeueAfter}`; **status reconstructed from the world, not read back as truth**. A validating
webhook enforces the (platform, transport) invariant (DESIGN-core §4.1) and validates `spec.config`
against `schema/ggcommons-config-schema.json`. The controller **fans out to and reconciles** a ConfigMap
(rendered + substituted config), the Deployment/StatefulSet (image, probes, PVC), an `ExternalSecret`
(delegated to ESO), and a `ServiceMonitor` (delegated to the Prometheus Operator) — it does **not**
reimplement secret sync or scrape config.

## 4. Why this ordering is safe

Deferring the operator costs nothing: the Helm chart is the supported path, and a future operator would
*wrap* the same rendered objects, so adopting it later is additive, not a rewrite. Conversely, building it
now would commit the project to a Go controller and CRD lifecycle before any stateful requirement justifies
it — exactly the speculative complexity the project's "simplicity first" rule warns against.
