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
| `split-config/` | Full split-config E2E harness: EMQX + Rust ConfigComponent + Java/Python/Rust/TypeScript skeletons + verifier Job. |
| `setup-runner-vm.sh` | Bootstrap script for a dedicated Ubuntu VM used as the repeatable local Kubernetes E2E runner. |
| `../../.github/workflows/k8s.yml` | CI job: kind + build/load image + helm install + `smoke.sh`. |

## The component config (ConfigMap)

`chart/templates/configmap.yaml` renders a **minimal valid edgecommons config** into the
`config.json` key (the `CONFIGMAP` source's default key), mounted at `/etc/edgecommons`:

- `metricEmission` namespace only; the KUBERNETES profile defaults the target to `prometheus`,
- a `heartbeat` (5s interval),
- `messaging.local` MQTT pointing at the in-cluster broker Service DNS
  (`edgecommons-emqx`, configurable via `messaging.brokerHost`),
- `component` (the only schema-required key).

Mounting the **whole volume** (not a `subPath`) is what lets the kubelet perform the
atomic `..data` swap that the `CONFIGMAP` source watches for hot-reload (FR-CFG-2/3).

## Set up a dedicated Kubernetes runner VM

For repeatable local E2E runs, use a dedicated Ubuntu VM rather than the Greengrass device or a
kind control-plane container as the build/deploy workstation. The bootstrap script installs Docker,
kind, kubectl, Helm, jq/yq, Java 25 LTS, Maven, Node.js 24 LTS, Python, Rust, and native build tools
with pinned tool defaults. It also configures the `prometheus-community` Helm repository so E2E runs
can install Prometheus into the disposable kind cluster.

```bash
bash test-infra/k8s/setup-runner-vm.sh
```

The script runs a short kind probe by default and deletes the probe cluster afterward. Set
`RUN_PROBE=0` to skip that check. If Docker's Ubuntu repository does not publish the pinned Docker
package for the VM's Ubuntu codename, the script prints the available package versions and stops so
the pin can be chosen explicitly.

Prometheus should be installed as a cluster add-on for the test run, not as a host-level daemon on
the VM. That keeps each run isolated and exercises the real Kubernetes `ServiceMonitor` scrape path:

```bash
helm upgrade --install kps prometheus-community/kube-prometheus-stack \
  --version 87.10.1 \
  --namespace monitoring \
  --create-namespace \
  --wait
```

When installing the EdgeCommons chart for a Prometheus-backed E2E run, enable the `ServiceMonitor`
and label it for the stack release:

```bash
helm upgrade --install ggc test-infra/k8s/chart \
  --namespace edgecommons \
  --set serviceMonitor.enabled=true \
  --set serviceMonitor.labels.release=kps
```

## Run it on kind

Prereqs: `docker`, `kind`, `kubectl`, `helm` (v3+).

```bash
# 1. Cluster
kind create cluster --name edgecommons --config test-infra/k8s/kind-config.yaml

# 2. Build the component image and load it into the cluster
docker build -f test-infra/k8s/Dockerfile -t edgecommons-component:ci .
docker save edgecommons-component:ci | docker exec -i edgecommons-control-plane ctr -n k8s.io images import -
docker exec edgecommons-control-plane crictl images | grep edgecommons-component

# 3. Run the smoke test (installs broker + chart, asserts everything, incl. hot-reload)
IMAGE=edgecommons-component:ci NAMESPACE=edgecommons ./test-infra/k8s/smoke.sh
```

The direct `ctr` import is intentional for the pinned `kindest/node:v1.36.1` node image. With kind
v0.30.0, `kind load docker-image` can fail against this node image with
`unknown containerd config version: 4`.

`smoke.sh` cleans up the namespace on exit. Use `KEEP=1` only while collecting diagnostics, then
delete the namespace when evidence has been captured.

## Run full split-config E2E

The smoke test above is not the full split-config acceptance gate. For split-config changes, run the
checked-in harness:

```bash
cd ~/source/edgecommons/core
bash test-infra/k8s/split-config/run.sh
```

For evidence collection:

```bash
cd ~/source/edgecommons/core
KEEP=1 bash test-infra/k8s/split-config/run.sh | tee /tmp/edgecommons-split-e2e.log
kubectl -n edgecommons-split get pods
kubectl -n edgecommons-split logs job/edgecommons-split-verifier
kubectl delete namespace edgecommons-split --wait=false
```

The harness builds local images from the current VM checkout and does not require a GitHub push. If
the code under test is local to the Windows workstation, copy or `rsync` the relevant source trees to
the runner VM first. The full run proves that ConfigComponent bootstraps from ConfigMap/file config,
all four skeletons bootstrap with `CONFIG_COMPONENT`, a volatile `update-catalog` message fans out
new split bundles, each skeleton dynamically reloads, and no split-config pod restarts.

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
- The `health` and `metrics` ports expose the live HTTP health endpoint and Prometheus metrics
  target used by the smoke and E2E tests.
