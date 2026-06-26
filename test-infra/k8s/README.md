# ggcommons on Kubernetes — Phase-1a test harness

This harness exercises the **KUBERNETES platform** and its native config mechanism — the
new **`CONFIGMAP` config source** with directory-watch hot-reload — on a real cluster
(kind locally, or lab k3s). It is the live counterpart to the unit tests that verify the
source + the simulated `..data` swap in each language library.

What Phase 1a proves end-to-end:

1. a pod **auto-detects `platform=KUBERNETES`** from its projected ServiceAccount token
   (`/var/run/secrets/kubernetes.io/serviceaccount/token`) — no `--platform` flag;
2. it loads its component config via the **`CONFIGMAP` source** from a ConfigMap mounted
   as a **directory** at `/etc/ggcommons` (never `subPath`);
3. it connects to the **in-cluster EMQX broker by Service DNS**;
4. a **`kubectl` edit of the ConfigMap is hot-reloaded in-process** — the watcher
   re-arms across the kubelet's atomic `..data` symlink swap — **with no pod restart**.

> Not in 1a (deferred, with TODO markers in the chart): the HTTP `/livez,/readyz` health
> endpoint and the `prometheus` metrics target (sub-phase **1c**); PVC-aware streaming,
> the `env` KeyProvider, and Downward-API identity (**1b/1d**). The chart's probes and
> Service are therefore **placeholders**.

## Contents

| Path | What it is |
|------|------------|
| `chart/` | Helm chart: Deployment, ConfigMap (the component `config.json`), ServiceAccount + optional namespaced RBAC, a placeholder Service, and placeholder liveness/readiness probes. |
| `kind-config.yaml` | Single-node kind cluster definition. |
| `emqx.yaml` | In-cluster EMQX MQTT broker (Deployment + ClusterIP Service `ggcommons-emqx`, plaintext 1883). |
| `Dockerfile` | Builds the default (Python) component image used by the smoke test. |
| `smoke.sh` | Assertion script the **orchestrator/CI runs live** (installs everything, asserts the four points above incl. the hot-reload test). |
| `../../.github/workflows/k8s.yml` | CI job: kind + build/load image + helm install + `smoke.sh`. |

## The component config (ConfigMap)

`chart/templates/configmap.yaml` renders a **minimal valid ggcommons config** into the
`config.json` key (the `CONFIGMAP` source's default key), mounted at `/etc/ggcommons`:

- `metricEmission.target: log` (a log metric target),
- a `heartbeat` (5s interval, metric target),
- `messaging.local` MQTT pointing at the in-cluster broker Service DNS
  (`ggcommons-emqx`, configurable via `messaging.brokerHost`),
- `component` (the only schema-required key).

Mounting the **whole volume** (not a `subPath`) is what lets the kubelet perform the
atomic `..data` swap that the `CONFIGMAP` source watches for hot-reload (FR-CFG-2/3).

## Run it on kind

Prereqs: `docker`, `kind`, `kubectl`, `helm` (v3+).

```bash
# 1. Cluster
kind create cluster --name ggcommons --config test-infra/k8s/kind-config.yaml

# 2. Build the component image and load it into the cluster
docker build -f test-infra/k8s/Dockerfile -t ggcommons-component:ci .
kind load docker-image ggcommons-component:ci --name ggcommons

# 3. Run the smoke test (installs broker + chart, asserts everything, incl. hot-reload)
IMAGE=ggcommons-component:ci NAMESPACE=ggcommons ./test-infra/k8s/smoke.sh
```

`smoke.sh` cleans up the namespace on exit; pass `KEEP=1` to leave it for inspection.

## Run it on lab k3s

k3s already has a cluster, so skip `kind`. Push/import the image to a registry the nodes
can reach (or `k3s ctr images import` a saved tar), then point the chart at it:

```bash
# Build + import the image into k3s's containerd
docker build -f test-infra/k8s/Dockerfile -t ggcommons-component:ci .
docker save ggcommons-component:ci | sudo k3s ctr images import -

# Run the smoke test against the current kubecontext
IMAGE=ggcommons-component:ci NAMESPACE=ggcommons ./test-infra/k8s/smoke.sh
```

## The ConfigMap hot-reload test (the `..data` re-arm)

`smoke.sh` performs it automatically, but to do it by hand:

```bash
# Watch the component logs
kubectl -n ggcommons logs -l app.kubernetes.io/instance=ggc -f

# In another shell, edit the ConfigMap (flip logging.level, or change any value)
kubectl -n ggcommons edit configmap ggc-ggcommons-component-config
```

Within the kubelet's sync window (~60–90s at defaults) the running pod logs an
**in-process reload** (e.g. `ConfigMap changed` / `configuration reloaded`) and
`restartCount` stays `0` — the watcher survived the inode replacement by re-arming after
the `..data` symlink swap. An **invalid** edit is **rejected-and-kept**: the pod logs a
validation warning and keeps serving the previous config (FR-CFG-5).

> `subPath` mounts never receive the `..data` swap, so hot-reload is silently dead — the
> chart always mounts the whole volume, and the `CONFIGMAP` source warns when it detects
> a mount with no `..data` link (FR-CFG-3).

## Static validation (no cluster)

```bash
helm lint     test-infra/k8s/chart
helm template ggc test-infra/k8s/chart        # render with defaults
helm template ggc test-infra/k8s/chart --set rbac.create=true   # render the optional RBAC
```

## Notes & knobs

- `values.yaml` parameterizes `image`, the broker Service DNS (`messaging.brokerHost`),
  and `replicas`.
- RBAC is **off by default** — the component needs no Kubernetes API access (config and
  secrets arrive as mounted volumes). Enable least-privilege, ConfigMap-scoped read RBAC
  with `--set rbac.create=true`.
- The `health`/`metrics` ports and the Service are wired now but have **no live
  listeners until sub-phase 1c** adds the HTTP health endpoint and the prometheus target.
