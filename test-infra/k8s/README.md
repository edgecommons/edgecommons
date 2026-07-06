# edgecommons on Kubernetes — Phase-1a/1b test harness

This harness exercises the **KUBERNETES platform** and its native facilities — the
**`CONFIGMAP` config source** with directory-watch hot-reload (1a), the
**MQTT-broker-config-from-ConfigMap convention** and **Downward-API identity** (1b) — on a
real cluster (kind locally, or lab k3s). It is the live counterpart to the unit tests that
verify the source + the simulated `..data` swap in each language library.

What the harness proves end-to-end:

1. a pod **auto-detects `platform=KUBERNETES`** from its projected ServiceAccount token
   (`/var/run/secrets/kubernetes.io/serviceaccount/token`) — no `--platform` flag needed;
2. it loads its component config via the **`CONFIGMAP` source** from a ConfigMap mounted
   as a **directory** at `/etc/edgecommons` (never `subPath`);
3. it connects to the **in-cluster EMQX broker by Service DNS** — and on KUBERNETES the
   broker config is **sourced from the mounted ConfigMap with no positional `--transport MQTT
   <path>`** (Phase 1b, FR-MSG-1): the single `config.json` carries both the `messaging`
   section and the component config;
4. with **no `-t/--thing`**, the component's **identity resolves from the Downward API**
   (Phase 1b, FR-RT-7): `EDGECOMMONS_THING_NAME` (set from `values.thingName`) ▸ `POD_NAME`
   (the pod's `metadata.name`, injected via a `fieldRef`);
5. it logs to a **structured stdout-JSON sink** (one JSON object per line — the KUBERNETES
   default, Phase 1c) carrying Downward-API **correlation fields**; the smoke asserts a JSON
   line whose `thing` correlation equals the pod's `POD_NAME`;
6. it serves an **HTTP health endpoint** (Phase 1c, on by default on KUBERNETES): the chart's
   `httpGet` startup/liveness/readiness probes hit `/startupz`,`/livez`,`/readyz` on `:8081`,
   so the pod only reaches **Ready** when `/readyz` returns 200 (messaging connected + ready);
   the smoke also GETs `/livez` in-pod to prove liveness is served and broker-independent;
7. it serves a **pull-based `prometheus` metrics endpoint** (Phase 1c, the default metric target on
   KUBERNETES): the component exposes an in-process registry as OpenMetrics text at `:9090/metrics`
   (no `metricEmission.target` in the config — the profile default applies), and the heartbeat is
   routed to it; the smoke does an in-pod GET of `/metrics` and asserts a `edgecommons_*` gauge appears;
8. it unlocks an **encrypted credentials vault via the `env` KeyProvider** (Phase 1d, the default
   vault custodian on KUBERNETES): `--set credentials.enabled=true` mounts a Secret as
   `EDGECOMMONS_VAULT_KEK` and injects a `credentials` section with `keyProvider` omitted (so the
   profile default `env` applies); the skeleton opens the vault from that base64 KEK and round-trips a
   demo secret — the smoke asserts the `credential access OK` log (offline, no cloud/HSM);
9. a **`kubectl` edit of the ConfigMap is hot-reloaded in-process** — the watcher
   re-arms across the kubelet's atomic `..data` symlink swap — **with no pod restart**
   (`restartCount=0` also confirms the liveness probe never failed).

> Also exercised (not asserted explicitly here): **SIGTERM → graceful shutdown** — the library
> wires SIGTERM to flip `/readyz`→503 then unsubscribe-all + bounded-close (FR-HB-2).
>
> **Scrape & central aggregation:** the `/metrics` scrape is **intra-cluster** (a local collector /
> the opt-in `ServiceMonitor` — enable with `--set serviceMonitor.enabled=true`), never cloud→edge
> inbound. Multi-site aggregation is the collector's **outbound** `remote_write` to AMP/Mimir/Thanos/
> Grafana Cloud — edge-initiated egress, same direction as CloudWatch (see DESIGN-subsystems §3.1).
>
> **Durable streaming on k8s:** a component using `gg.streams()` with a durable disk buffer needs a
> **StatefulSet + per-pod PVC** (single-writer), not a Deployment — see `streaming-statefulset-example.yaml`
> + DESIGN-packaging. The edgestreamlog engine is unchanged; this is a deployment shape (not smoke-tested
> here — it needs a reachable Kinesis/Kafka sink).
>
> Deferred metrics enhancements (measure `type`/histograms, a unified durable metrics-streamlog,
> heartbeat-as-collector)
> are captured in DESIGN-subsystems §3.2.

## Contents

| Path | What it is |
|------|------------|
| `chart/` | Helm chart: Deployment, ConfigMap (the component `config.json`), ServiceAccount + optional namespaced RBAC, a placeholder Service, and placeholder liveness/readiness probes. |
| `kind-config.yaml` | Single-node kind cluster definition. |
| `emqx.yaml` | In-cluster EMQX MQTT broker (Deployment + ClusterIP Service `edgecommons-emqx`, plaintext 1883). |
| `Dockerfile` | Builds the default (Python) component image used by the smoke test. |
| `smoke.sh` | Assertion script the **orchestrator/CI runs live** (installs everything, asserts the four points above incl. the hot-reload test). |
| `../../.github/workflows/k8s.yml` | CI job: kind + build/load image + helm install + `smoke.sh`. |

## The component config (ConfigMap)

`chart/templates/configmap.yaml` renders a **minimal valid edgecommons config** into the
`config.json` key (the `CONFIGMAP` source's default key), mounted at `/etc/edgecommons`:

- `metricEmission.target: log` (a log metric target),
- a `heartbeat` (5s interval, metric target),
- `messaging.local` MQTT pointing at the in-cluster broker Service DNS
  (`edgecommons-emqx`, configurable via `messaging.brokerHost`),
- `component` (the only schema-required key).

Mounting the **whole volume** (not a `subPath`) is what lets the kubelet perform the
atomic `..data` swap that the `CONFIGMAP` source watches for hot-reload (FR-CFG-2/3).

## Run it on kind

Prereqs: `docker`, `kind`, `kubectl`, `helm` (v3+).

```bash
# 1. Cluster
kind create cluster --name edgecommons --config test-infra/k8s/kind-config.yaml

# 2. Build the component image and load it into the cluster
docker build -f test-infra/k8s/Dockerfile -t edgecommons-component:ci .
kind load docker-image edgecommons-component:ci --name edgecommons

# 3. Run the smoke test (installs broker + chart, asserts everything, incl. hot-reload)
IMAGE=edgecommons-component:ci NAMESPACE=edgecommons ./test-infra/k8s/smoke.sh
```

`smoke.sh` cleans up the namespace on exit; pass `KEEP=1` to leave it for inspection.

## Run it on lab k3s

k3s already has a cluster, so skip `kind`. Push/import the image to a registry the nodes
can reach (or `k3s ctr images import` a saved tar), then point the chart at it:

```bash
# Build + import the image into k3s's containerd
docker build -f test-infra/k8s/Dockerfile -t edgecommons-component:ci .
docker save edgecommons-component:ci | sudo k3s ctr images import -

# Run the smoke test against the current kubecontext
IMAGE=edgecommons-component:ci NAMESPACE=edgecommons ./test-infra/k8s/smoke.sh
```

## The ConfigMap hot-reload test (the `..data` re-arm)

`smoke.sh` performs it automatically, but to do it by hand:

```bash
# Watch the component logs
kubectl -n edgecommons logs -l app.kubernetes.io/instance=ggc -f

# In another shell, edit the ConfigMap (flip logging.level, or change any value)
kubectl -n edgecommons edit configmap ggc-edgecommons-component-config
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
- **Identity** (FR-RT-7): the chart always injects `POD_NAME`/`POD_NAMESPACE`/`NODE_NAME`
  via the Downward API. Set `--set thingName=my-thing` to pin a stable identity (exposed as
  `EDGECOMMONS_THING_NAME`, the highest k8s identity tier); leave it empty to fall through to
  `POD_NAME`. Append further env (e.g. `AWS_REGION`) via `extraEnv`.
- **Messaging** (FR-MSG-1): the chart passes no `--transport MQTT <path>` — the KUBERNETES
  profile derives `MQTT` and the messaging-config path defaults to the mounted ConfigMap
  file. An explicit `--transport MQTT <path>` in `args` still overrides.
- RBAC is **off by default** — the component needs no Kubernetes API access (config and
  secrets arrive as mounted volumes). Enable least-privilege, ConfigMap-scoped read RBAC
  with `--set rbac.create=true`.
- The `health`/`metrics` ports and the Service are wired now but have **no live
  listeners until sub-phase 1c** adds the HTTP health endpoint and the prometheus target.
