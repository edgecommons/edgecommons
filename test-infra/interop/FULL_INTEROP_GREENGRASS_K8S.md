# Full Interop Runbook: Greengrass and Kubernetes

This runbook is the acceptance procedure for full EdgeCommons interoperability when a change affects
wire behavior, request/reply behavior, command topics, or hierarchical configuration. It is stricter
than a component smoke test: every skeleton component must run as a real deployed component, must use
the production platform transport and config path, and must emit evidence that the changed behavior
reached the component runtime.

The reader is expected to be an EdgeCommons maintainer with access to the repo, the language
toolchains, the `lab-5950x` Greengrass core, and a Kubernetes cluster such as kind or lab k3s.

## Pass Criteria

A full run is passing only when all of the following are true:

1. Java, Python, Rust, and TypeScript skeletons are deployed and running.
2. Each skeleton uses the intended platform transport:
   - Greengrass: `GREENGRASS` + `IPC`.
   - Kubernetes: `KUBERNETES` + `MQTT`.
3. Each skeleton uses hierarchical config from `com.mbreissi.edgecommons.ConfigComponent` with
   `-c CONFIG_COMPONENT`.
4. The ConfigComponent bootstraps from a non-`CONFIG_COMPONENT` source:
   - Greengrass: `GG_CONFIG` with its own `ComponentConfig`.
   - Kubernetes: mounted ConfigMap file source.
5. A non-production `update-catalog` message is sent to the ConfigComponent.
6. The ConfigComponent acknowledges the update. For the message-update path the acknowledgement must
   identify volatile provenance (`source=message`, `interface=update-catalog`, `volatile=true`).
   The verifier must also observe `pushed_count: 4`.
7. Every pushed bundle is a lineage bundle:
   - `lineageVersion: 1`.
   - `catalogVersion` equals the updated catalog version.
   - No top-level `base`.
   - `layers[]` is ordered root-to-component.
   - Layer ids include `enterprise/...`, `site/...`, `zone/...`, `line/...`,
     and `component/<lookup-token>`.
8. Every skeleton logs that it received and dynamically reloaded the update.
9. The update is volatile: the catalog file or ConfigMap remains unchanged after the message update.
10. Request/reply and publish/subscribe are exercised through the skeletons or interop nodes, not
    only by checking that components are `RUNNING`.

Do not report a run complete when only the ConfigComponent observed the push. A consumer can miss a
push if it has not yet subscribed to its `set-config` inbox. Kubernetes `Ready` and Greengrass
`RUNNING` are necessary startup signals, but they are not sufficient evidence that hierarchical
bootstrap and `set-config` subscriptions have completed.

The standard validation hierarchy is:

```text
enterprise -> site -> zone -> line -> device
```

The catalog defines scopes through `line`. The runtime device identity is the Greengrass thing or
Kubernetes test thing, and is not a catalog node.

## Greengrass Topology

```mermaid
flowchart LR
    subgraph dev["Developer workstation"]
        repo["edgecommons/core checkout"]
        build["Build language artifacts"]
        verifier["HierarchicalConfigVerifier / interop node"]
    end

    subgraph gg["lab-5950x Greengrass core"]
        nucleus["Greengrass Nucleus"]
        ipc["Local IPC pub/sub"]
        cfg["com.mbreissi.edgecommons.ConfigComponent\n-c GG_CONFIG"]
        catalog["catalog.json\nfile source cache"]
        java["JavaComponentSkeleton\n-c CONFIG_COMPONENT"]
        py["PythonComponentSkeleton\n-c CONFIG_COMPONENT"]
        rust["RustComponentSkeleton\n-c CONFIG_COMPONENT"]
        ts["TsComponentSkeleton\n-c CONFIG_COMPONENT"]
        logs["/greengrass/v2/logs"]
    end

    repo --> build --> nucleus
    verifier --> nucleus
    nucleus --> ipc
    cfg --> catalog
    cfg <--> ipc
    java <--> ipc
    py <--> ipc
    rust <--> ipc
    ts <--> ipc
    java --> logs
    py --> logs
    rust --> logs
    ts --> logs
    cfg --> logs
```

### Greengrass Message Flow

```mermaid
sequenceDiagram
    participant V as HierarchicalConfigVerifier
    participant C as ConfigComponent
    participant J as Java skeleton
    participant P as Python skeleton
    participant R as Rust skeleton
    participant T as TypeScript skeleton

    J->>C: get-configuration
    P->>C: get-configuration
    R->>C: get-configuration
    T->>C: get-configuration
    C-->>J: lineage bundle
    C-->>P: lineage bundle
    C-->>R: lineage bundle
    C-->>T: lineage bundle
    J->>J: subscribe set-config inbox
    P->>P: subscribe set-config inbox
    R->>R: subscribe set-config inbox
    T->>T: subscribe set-config inbox
    V->>C: update-catalog
    C-->>V: ack {ok:true, version}
    C-->>J: set-config lineage bundle
    C-->>P: set-config lineage bundle
    C-->>R: set-config lineage bundle
    C-->>T: set-config lineage bundle
    J->>J: merge layers and reload
    P->>P: merge layers and reload
    R->>R: merge layers and reload
    T->>T: merge layers and reload
```

## Greengrass Procedure

Run these commands from `core/` unless another directory is shown.

### 1. Build the host-side artifacts

```powershell
mvn -f libs/java/pom.xml -DskipTests install
mvn -f examples/java/pom.xml -DskipTests clean package

npm install

Push-Location libs/ts
npm run build
Pop-Location

Push-Location test-infra/interop/ts_node
npm run build
Pop-Location

Push-Location examples/ts
npm install
npm run build
Pop-Location

$jar = Get-ChildItem libs/java/target/edgecommons-*.jar |
  Where-Object { -not $_.Name.StartsWith('original-') -and -not $_.Name.EndsWith('-sources.jar') -and -not $_.Name.EndsWith('-javadoc.jar') } |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1
javac -cp $jar.FullName -d test-infra/interop/java_node/out test-infra/interop/java_node/InteropNode.java
```

Build the Linux Greengrass Rust binaries from WSL. `config-component` is the sibling repo
`../config-component`, not a directory under `core/`.

```powershell
wsl.exe bash -lc "cd /mnt/c/Users/breis/source/edgecommons/core/test-infra/interop/rust_node && CARGO_TARGET_DIR=/mnt/c/Users/breis/source/edgecommons/core/build/gg-rust-target cargo build --release --no-default-features --features greengrass"
wsl.exe bash -lc "cd /mnt/c/Users/breis/source/edgecommons/core/examples/rust && CARGO_TARGET_DIR=/mnt/c/Users/breis/source/edgecommons/core/build/gg-rust-skeleton-target cargo build --release --no-default-features --features greengrass"
wsl.exe bash -lc "cd /mnt/c/Users/breis/source/edgecommons/config-component && CARGO_TARGET_DIR=/mnt/c/Users/breis/source/edgecommons/core/build/gg-configcomponent-target cargo build --release --no-default-features --features greengrass"
```

Package the Greengrass IPC interop nodes:

```powershell
$ipcPackage = .\test-infra\interop\gg_ipc\package.ps1 `
  -RunId "full-interop-$(Get-Date -Format yyyyMMddHHmmss)" `
  -Langs "python,java,rust,ts"
```

The IPC package defaults to the binary body matrix. For the structured log bus matrix, pass the
explicit role:

```powershell
$logPackage = .\test-infra\interop\gg_ipc\package.ps1 `
  -RunId "log-$(Get-Date -Format yyyyMMddHHmmss)" `
  -Langs "python,java,rust,ts" `
  -Role gg-log-matrix
```

The `gg-log-matrix` role uses the same packaged Java, Python, Rust, RustPeer, and TypeScript
interop components, but each participant publishes one runtime structured log record and subscribes
to the UNS `ecv1/<device>/+/main/log/warn` stream. Each participant writes a result file named
`/tmp/edgecommons_gg_ipc_log_<ready-lang>_<run-id>.json`; the run passes only when every result has
`ok:true`, `missing:[]`, no errors, and received records from Java, Python, Rust, and TypeScript.

Package the P1 deferred-command and strict-confirmed-publish matrix separately. This role has its
own component names so it cannot overlap a binary or log validation deployment:

```powershell
$p1Package = .\test-infra\interop\gg_ipc\package.ps1 `
  -RunId "p1-$(Get-Date -Format yyyyMMddHHmmss)" `
  -Langs "python,java,rust,ts" `
  -Role gg-p1-matrix
```

The role deploys Java, Python, Rust, TypeScript, and a second Rust principal (`RustPeer`). The
logical test matrix remains exactly four-by-four: `RustPeer` is only the receiving principal for
the Rust-to-Rust edge, avoiding an unobservable same-process self-delivery. Every actor writes
`/tmp/edgecommons_gg_ipc_p1_<actor>_<run-id>.json`; the packaged
`verify-p1-results.py` consumes all five files and rejects a missing, duplicate, uncorrelated, or
unconfirmed edge. Each responder fsyncs a bounded uniquely named local acceptance marker before
activating its deferred token and removes that marker only after terminal settlement is attempted.

Package the hierarchical ConfigComponent, four skeletons, catalogs, and one-shot verifier:

```powershell
$hierPackage = .\test-infra\interop\gg_hierarchical_config\package.ps1 `
  -RunId "hierarchical-config-$(Get-Date -Format yyyyMMddHHmmss)"
```

The package scripts print `RecipeDir`, `ArtifactDir`, `RunId`, and `Version`. Preserve those values.
For the skeleton/ConfigComponent deployment use `$hierPackage.Version`; for the binary matrix
deployment use `$ipcPackage.Version`.

### 2. Stage recipes and artifacts on the Greengrass core

Use a run-specific remote directory.

```powershell
$remote = "/tmp/edgecommons-full-interop-$($hierPackage.RunId)"
$ggHost = "marc@192.168.1.229"

ssh $ggHost "/greengrass/v2/bin/greengrass-cli --help | head -5"
ssh $ggHost "sudo -n true"
ssh $ggHost "mkdir -p $remote/recipes $remote/artifacts /tmp/edgecommons-full-interop"
scp -r "$($ipcPackage.RecipeDir)" "$($ggHost):$remote/ipc-recipes"
scp -r "$($hierPackage.RecipeDir)" "$($ggHost):$remote/hier-recipes"
scp -r "$($ipcPackage.ArtifactDir)" "$($ggHost):$remote/ipc-artifacts"
scp -r "$($hierPackage.ArtifactDir)" "$($ggHost):$remote/hier-artifacts"
ssh $ggHost "cp $remote/ipc-recipes/* $remote/recipes/ && cp $remote/hier-recipes/* $remote/recipes/ && cp -R $remote/ipc-artifacts/* $remote/artifacts/ && cp -R $remote/hier-artifacts/* $remote/artifacts/"
scp "$($hierPackage.CatalogInitial)" "$($ggHost):/tmp/edgecommons-full-interop/catalog-initial.json"
scp "$($hierPackage.CatalogUpdate)" "$($ggHost):/tmp/edgecommons-full-interop/catalog-update-second-pass.json"
scp "$($hierPackage.ConfigComponentUpdate)" "$($ggHost):$remote/configcomponent-update.json"
```

If a combined staging block appears to hang from PowerShell, run the `scp` commands one at a time
and verify counts before continuing:

```powershell
ssh $ggHost "find $remote/recipes -maxdepth 1 -type f | wc -l && find $remote/artifacts -type f | wc -l"
```

The expected counts are 11 recipes and 11 artifact files for the full Greengrass hierarchical-config
plus IPC verifier package set.

The `sudo -n true` preflight must return immediately. If it hangs or reports that a password is
required, stop and fix the Greengrass operator session before continuing; deployment and log
collection steps use `sudo` and cannot be validated through a non-interactive runbook while sudo is
blocked.

The generated skeleton recipes run the components with:

```text
--platform GREENGRASS -c CONFIG_COMPONENT
```

The ConfigComponent recipe must run with:

```text
--platform GREENGRASS -c GG_CONFIG
```

and its own deployment configuration must include `ComponentConfig`:

```json
{
  "ComponentConfig": {
    "component": {
      "token": "edgecommons-config-component",
      "global": {
        "configComponent": {
          "catalogSource": {
            "type": "file",
            "path": "/greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json",
            "watch": true
          },
          "pushOnCatalogReload": true,
          "allowVolatileCatalogUpdates": true
        }
      },
      "instances": []
    }
  }
}
```

`allowVolatileCatalogUpdates:true` is for this validation run only.

### 3. Install the initial catalog

The generated initial catalog must include hierarchy levels, four scope nodes, and one component
entry for every skeleton:

```text
hierarchy.levels = ["enterprise","site","zone","line","device"]
nodes = enterprise/acme -> site/integration-lab -> zone/greengrass-zone -> line/line-7
components = JavaComponentSkeleton, PythonComponentSkeleton, RustComponentSkeleton, TsComponentSkeleton
```

Copy it into the ConfigComponent work directory before deployment:

```powershell
ssh $ggHost "sudo mkdir -p /greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent && sudo cp /tmp/edgecommons-full-interop/catalog-initial.json /greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json && sudo chmod 644 /greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json"
```

### 4. Deploy ConfigComponent and all skeletons

Deploy the ConfigComponent and the four skeletons in one local deployment. Use the run-specific
versions you staged.

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli deployment create --recipeDir $remote/recipes --artifactDir $remote/artifacts --update-config $remote/configcomponent-update.json --merge com.mbreissi.edgecommons.ConfigComponent=$($hierPackage.Version) --merge com.mbreissi.edgecommons.JavaComponentSkeleton=$($hierPackage.Version) --merge com.mbreissi.edgecommons.PythonComponentSkeleton=$($hierPackage.Version) --merge com.mbreissi.edgecommons.RustComponentSkeleton=$($hierPackage.Version) --merge com.mbreissi.edgecommons.TsComponentSkeleton=$($hierPackage.Version)"
```

Wait until all five components are running:

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli component list | grep -E 'ConfigComponent|Skeleton' -A5 -B1"
```

Required evidence:

- ConfigComponent is `RUNNING`.
- All four skeletons are `RUNNING`.
- ConfigComponent effective config shows `allowVolatileCatalogUpdates:true`.
- Each skeleton log shows `configSource=CONFIG_COMPONENT`.
- Each skeleton has completed initial hierarchical bootstrap before the update is sent.
- The verifier or operator log shows all four `set-config` subscriptions are active before the
  `update-catalog` request is published.

### 5. Prove baseline request/reply and pub/sub

Deploy the packaged Greengrass IPC interop nodes if the change affects message wire behavior:

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli deployment create --recipeDir $remote/recipes --artifactDir $remote/artifacts --merge com.mbreissi.edgecommons.InteropBinaryPython=$($ipcPackage.Version) --merge com.mbreissi.edgecommons.InteropBinaryJava=$($ipcPackage.Version) --merge com.mbreissi.edgecommons.InteropBinaryRust=$($ipcPackage.Version) --merge com.mbreissi.edgecommons.InteropBinaryTs=$($ipcPackage.Version)"
```

Required evidence:

- Each interop component exits successfully or logs a completed matrix.
- Every ordered producer/consumer pair is covered for request/reply.
- Binary payload tests prove byte-for-byte body preservation when that behavior is in scope.

### P1 deferred-command and strict-publish IPC matrix

Run this independently when the change affects deferred command settlement or strict confirmed
publication. Do not substitute the local MQTT P1 matrix: this procedure exercises the real
Greengrass IPC component principals.

```powershell
$p1Remote = "/tmp/edgecommons-p1-$($p1Package.RunId)"
$p1Evidence = Join-Path $p1Package.OutputRoot "evidence"
New-Item -ItemType Directory -Force -Path $p1Evidence | Out-Null

ssh $ggHost "mkdir -p $p1Remote/recipes $p1Remote/artifacts"
scp -r "$($p1Package.RecipeDir)" "$($ggHost):$p1Remote/recipes"
scp -r "$($p1Package.ArtifactDir)" "$($ggHost):$p1Remote/artifacts"
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli deployment create --recipeDir $p1Remote/recipes --artifactDir $p1Remote/artifacts --merge com.mbreissi.edgecommons.InteropP1Python=$($p1Package.Version) --merge com.mbreissi.edgecommons.InteropP1Java=$($p1Package.Version) --merge com.mbreissi.edgecommons.InteropP1Rust=$($p1Package.Version) --merge com.mbreissi.edgecommons.InteropP1RustPeer=$($p1Package.Version) --merge com.mbreissi.edgecommons.InteropP1Ts=$($p1Package.Version)"
```

Wait for all five role result files, then copy and verify them on the build machine. The verifier
is the pass/fail authority; a component merely reaching `RUNNING` is not evidence of the matrix.

```powershell
ssh $ggHost "for f in /tmp/edgecommons_gg_ipc_p1_*_$($p1Package.RunId).json; do test -f \"`$f\" || exit 1; done; mkdir -p /tmp/edgecommons-p1-results-$($p1Package.RunId); cp /tmp/edgecommons_gg_ipc_p1_*_$($p1Package.RunId).json /tmp/edgecommons-p1-results-$($p1Package.RunId)/"
scp -r "$($ggHost):/tmp/edgecommons-p1-results-$($p1Package.RunId)/." $p1Evidence
py -3.14 $p1Package.ResultVerifier --directory $p1Evidence --run-id $p1Package.RunId
```

The verifier must emit `ok:true`, 16 deferred-command edges, and 16 strict confirmed-publish
edges. For every deferred edge it verifies the actual terminal reply body: request token,
responder language, responder actor, and `durablyAccepted:true`; it also requires the request
correlation to match and exactly one reply observed through the 750 ms duplicate window. Strict
publish evidence additionally proves target actor, QoS 1, and completion of the strict publish
API.

Always remove this validation deployment and its temporary files after evidence has been copied:

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli deployment create --remove=com.mbreissi.edgecommons.InteropP1Python --remove=com.mbreissi.edgecommons.InteropP1Java --remove=com.mbreissi.edgecommons.InteropP1Rust --remove=com.mbreissi.edgecommons.InteropP1RustPeer --remove=com.mbreissi.edgecommons.InteropP1Ts"
ssh $ggHost "rm -rf $p1Remote /tmp/edgecommons-p1-results-$($p1Package.RunId) /tmp/edgecommons_gg_ipc_p1_*_$($p1Package.RunId).json"
```

### 6. Send a second-pass catalog update

Do not send the update immediately after deployment. First confirm every skeleton has initialized and
subscribed. Then use `interop-rust-node gg-config-update-file` as the verifier:

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli deployment create --recipeDir $remote/recipes --artifactDir $remote/artifacts --merge com.mbreissi.edgecommons.HierarchicalConfigVerifier=$($hierPackage.Version)"
```

The verifier must subscribe to all four `set-config` topics before it sends the request:

```text
ecv1/lab-5950x/JavaComponentSkeleton/main/cmd/set-config
ecv1/lab-5950x/PythonComponentSkeleton/main/cmd/set-config
ecv1/lab-5950x/RustComponentSkeleton/main/cmd/set-config
ecv1/lab-5950x/TsComponentSkeleton/main/cmd/set-config
```

### 7. Collect Greengrass evidence

Read the verifier result:

```powershell
ssh $ggHost "sudo cat /tmp/edgecommons_full_interop/update-result.json"
```

Required fields:

```json
{
  "ok": true,
  "ack_ok": true,
  "correlation_match": true,
  "expected_pushes": 4,
  "pushed_count": 4
}
```

For each skeleton key under `push_checks`, require:

```json
{
  "ok": true,
  "catalogVersion": "second-pass-hierarchical-full-interop",
  "layerIds": [
    "enterprise/acme",
    "site/integration-lab",
    "zone/greengrass-zone",
    "line/line-7",
    "component/JavaComponentSkeleton"
  ]
}
```

The component layer id changes by skeleton. The merged effective config must also show:

- `identity.enterprise = "acme"`.
- `identity.site = "integration-lab"`.
- `identity.zone = "greengrass-zone"`.
- `identity.line = "line-7"`.
- `tags.lineageMarker = "gg-hierarchical-second-pass"`.
- Updated per-component `component.global.publish_interval` values:
  - Java: `21`.
  - Python: `23`.
  - Rust: `29`.
  - TypeScript: `31`.

Read component logs:

```powershell
ssh $ggHost "sudo grep -R --line-number -e 'configSource=CONFIG_COMPONENT' -e 'configuration changed' -e 'Publish interval changed' -e 'set-config push received' -e 'updated publish interval' /greengrass/v2/logs/com.mbreissi.edgecommons.*Skeleton*.log"
```

Expected evidence:

| Component | Required log evidence |
| --- | --- |
| Java | `configSource=CONFIG_COMPONENT`; `Publish interval changed ... to 21000ms` |
| Python | `configSource=CONFIG_COMPONENT`; `set-config push received`; reload hooks fire |
| Rust | `configSource=CONFIG_COMPONENT`; `configuration reloaded`; `updated publish interval to 29s` |
| TypeScript | `configSource=CONFIG_COMPONENT`; `configuration changed; updated publish interval to 31s` |

Finally, prove volatility:

```powershell
ssh $ggHost "sudo grep -n 'gg-hierarchical-second-pass' /greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json || true"
```

The grep must produce no match. The message update must not persist to the catalog file.

### 8. Remove validation components

After evidence is captured, remove the validation-only local deployment roots. Do not leave the
ConfigComponent, skeletons, verifier, or binary interop nodes running on a shared Greengrass core.

Use separate `--remove` arguments. On Nucleus 2.17, a comma-separated `--remove` value can be
recorded as one literal component name and remove nothing.

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli deployment create --remove=com.mbreissi.edgecommons.ConfigComponent --remove=com.mbreissi.edgecommons.JavaComponentSkeleton --remove=com.mbreissi.edgecommons.PythonComponentSkeleton --remove=com.mbreissi.edgecommons.RustComponentSkeleton --remove=com.mbreissi.edgecommons.TsComponentSkeleton --remove=com.mbreissi.edgecommons.HierarchicalConfigVerifier --remove=com.mbreissi.edgecommons.InteropBinaryPython --remove=com.mbreissi.edgecommons.InteropBinaryJava --remove=com.mbreissi.edgecommons.InteropBinaryRust --remove=com.mbreissi.edgecommons.InteropBinaryRustPeer --remove=com.mbreissi.edgecommons.InteropBinaryTs --remove=com.mbreissi.edgecommons.InteropP1Python --remove=com.mbreissi.edgecommons.InteropP1Java --remove=com.mbreissi.edgecommons.InteropP1Rust --remove=com.mbreissi.edgecommons.InteropP1RustPeer --remove=com.mbreissi.edgecommons.InteropP1Ts"
```

Verify that only baseline Greengrass services remain:

```powershell
ssh $ggHost "sudo /greengrass/v2/bin/greengrass-cli component list"
ssh $ggHost "ps -eo pid,ppid,user,stat,etime,cmd | grep -E 'com.mbreissi.edgecommons|InteropBinary|ConfigComponent|Skeleton' | grep -v grep || true"
```

## Kubernetes Topology

```mermaid
flowchart TB
    subgraph cluster["Kubernetes namespace: edgecommons-hierarchical"]
        emqx["EMQX Service\nedgecommons-emqx:1883"]
        catalog_cm["ConfigMap\ncatalog.json"]
        cfg_pod["ConfigComponent Pod\nmounted ConfigMap file source"]
        j_pod["Java skeleton Pod\n-c CONFIG_COMPONENT"]
        p_pod["Python skeleton Pod\n-c CONFIG_COMPONENT"]
        r_pod["Rust skeleton Pod\n-c CONFIG_COMPONENT"]
        t_pod["TypeScript skeleton Pod\n-c CONFIG_COMPONENT"]
        verifier_pod["Verifier Job/Pod"]
    end

    catalog_cm --> cfg_pod
    cfg_pod <--> emqx
    j_pod <--> emqx
    p_pod <--> emqx
    r_pod <--> emqx
    t_pod <--> emqx
    verifier_pod <--> emqx
```

## Kubernetes Procedure

The checked-in `test-infra/k8s/smoke.sh` is the single-component CONFIGMAP smoke. It is still useful
for proving the Kubernetes platform profile, ConfigMap hot-reload, health, metrics, and the Helm
chart. It is not the full hierarchical-config acceptance gate.

Full Kubernetes hierarchical-config E2E is implemented by
`test-infra/k8s/hierarchical-config/run.sh`. That harness builds local images, loads them into kind,
deploys EMQX, deploys the Rust ConfigComponent, deploys the Java/Python/Rust/TypeScript skeletons
with `-c CONFIG_COMPONENT`, runs a verifier Job, proves message-based volatile catalog update
fanout, and checks that no hierarchical-config pod restarted.

### 1. Create or select a cluster

Preflight the tools in the shell where the Kubernetes commands will run. For repeatable end-to-end
system tests, use the dedicated Kubernetes runner VM instead of the Greengrass device or a
pre-existing control-plane utility container.

```bash
kubectl version --client=true
helm version --short
docker version --format '{{.Client.Version}}'
kind version || true
```

For a dedicated Ubuntu VM runner, bootstrap those prerequisites with:

```bash
bash test-infra/k8s/setup-runner-vm.sh
```

For kind:

```bash
kind create cluster --name edgecommons --config test-infra/k8s/kind-config.yaml
kubectl config use-context kind-edgecommons
```

For lab k3s, select the lab kubecontext and skip `kind`.

### 2. Synchronize the code under test

The Kubernetes hierarchical-config harness can validate local in-progress code. It does not require
a GitHub push. The runner only needs the same local source state that is being validated.

Expected runner layout:

```text
~/source/edgecommons/core
~/source/edgecommons/config-component
```

If the VM clone already has the target commits, update it normally:

```bash
cd ~/source/edgecommons/core && git pull --ff-only
cd ~/source/edgecommons/config-component && git pull --ff-only
```

If the code is not pushed, copy the local source slices to the VM. At minimum, synchronize the
language libraries, skeleton examples, protobuf sources, hierarchical-config vectors,
ConfigComponent, and `test-infra/k8s/hierarchical-config/`.

From a WSL shell on the Windows workstation, `rsync` is the least error-prone option:

```bash
VM=edgecommons-k8s
ROOT=/mnt/c/Users/breis/source/edgecommons

rsync -az --delete "$ROOT/core/libs/java/" "$VM:~/source/edgecommons/core/libs/java/"
rsync -az --delete "$ROOT/core/libs/python/" "$VM:~/source/edgecommons/core/libs/python/"
rsync -az --delete "$ROOT/core/libs/rust/" "$VM:~/source/edgecommons/core/libs/rust/"
rsync -az --delete "$ROOT/core/libs/rust-streamlog/" "$VM:~/source/edgecommons/core/libs/rust-streamlog/"
rsync -az --delete "$ROOT/core/libs/ts/" "$VM:~/source/edgecommons/core/libs/ts/"
rsync -az --delete "$ROOT/core/examples/" "$VM:~/source/edgecommons/core/examples/"
rsync -az --delete "$ROOT/core/proto/" "$VM:~/source/edgecommons/core/proto/"
rsync -az --delete "$ROOT/core/hierarchical-config-test-vectors/" "$VM:~/source/edgecommons/core/hierarchical-config-test-vectors/"
rsync -az --delete "$ROOT/core/test-infra/k8s/hierarchical-config/" "$VM:~/source/edgecommons/core/test-infra/k8s/hierarchical-config/"
rsync -az --delete "$ROOT/config-component/" "$VM:~/source/edgecommons/config-component/"
```

If `rsync` is unavailable, use `scp -r` for the same directories. Be explicit about the source trees
instead of copying build output directories wholesale.

### 3. Run the full hierarchical-config harness

Run from the VM:

```bash
cd ~/source/edgecommons/core
bash -n test-infra/k8s/hierarchical-config/run.sh
bash test-infra/k8s/hierarchical-config/run.sh | tee /tmp/edgecommons-hierarchical-e2e.log
```

For evidence collection or failure diagnostics, keep the namespace and write a durable log:

```bash
cd ~/source/edgecommons/core
KEEP=1 bash test-infra/k8s/hierarchical-config/run.sh | tee /tmp/edgecommons-hierarchical-e2e.log
```

The harness defaults are:

- namespace: `edgecommons-hierarchical`.
- kind cluster: `edgecommons`.
- device identity: `edgecommons-k8s-line-7`.
- initial lineage marker: `k8s-hierarchical-initial`.
- updated lineage marker: `k8s-hierarchical-updated`.
- initial per-component `publish_interval`: `5`.
- updated per-component `publish_interval`: `1`.

The harness builds and loads these local images:

- `edgecommons-config-component:hierarchical-ci`.
- `edgecommons-java-skeleton:hierarchical-ci`.
- `edgecommons-python-skeleton:hierarchical-ci`.
- `edgecommons-rust-skeleton:hierarchical-ci`.
- `edgecommons-ts-skeleton:hierarchical-ci`.
- `edgecommons-hierarchical-verifier:hierarchical-ci`.

### 4. What the Kubernetes harness proves

The ConfigComponent pod bootstraps from a mounted ConfigMap/file source, not from
`CONFIG_COMPONENT`. Its bootstrap config includes:

```text
catalogSource.type=configmap
catalogSource.mountDir=/etc/edgecommons/config
catalogSource.key=catalog.json
pushOnCatalogReload=true
allowVolatileCatalogUpdates=true
```

The skeleton pods all run with:

```text
--platform KUBERNETES --transport MQTT /etc/edgecommons/bootstrap/<lang>-messaging.json -c CONFIG_COMPONENT -t edgecommons-k8s-line-7
```

The verifier Job:

- sends `GetConfiguration` requests to
  `ecv1/edgecommons-k8s-line-7/config/main/cmd/get-configuration`;
- proves the initial lineage bundles contain ordered
  `enterprise/acme`, `site/integration-lab`, `zone/k8s-zone`, `line/line-7`,
  and `component/<component-key>` layers;
- proves merged effective config contains `tags.lineageMarker=k8s-hierarchical-initial`,
  schema-valid component tokens, each language's unique component marker, and
  `publish_interval=5`;
- subscribes to all four `set-config` inboxes;
- sends `UpdateCatalog` to `ecv1/edgecommons-k8s-line-7/config/main/cmd/update-catalog`;
- requires an acknowledgement with `ok=true` and captures the acknowledgement provenance;
- proves the pushed bundles contain `tags.lineageMarker=k8s-hierarchical-updated` and
  `publish_interval=1`;
- waits for dynamic reload evidence from all four skeletons;
- verifies all hierarchical-config pod restart counts are zero.

The ConfigComponent must not write the message-delivered catalog back to the ConfigMap. The message
update path is for non-production debug, verification, and test use only; it is intentionally
volatile and is lost when the ConfigComponent restarts.

### 5. Required Kubernetes evidence

```bash
kubectl -n edgecommons-hierarchical get pods
kubectl -n edgecommons-hierarchical logs job/edgecommons-hierarchical-verifier
kubectl -n edgecommons-hierarchical logs -l app.kubernetes.io/part-of=edgecommons-hierarchical-config --all-containers --tail=-1
```

Required harness log evidence:

- `verifier proved initial get-configuration and update-catalog fanout`.
- `ConfigComponent pushed updated hierarchical catalog bundles`.
- `Java skeleton dynamically reloaded updated component config`.
- `Python skeleton received set-config and notified listeners`.
- `Rust skeleton dynamically reloaded updated component config`.
- `TypeScript skeleton dynamically reloaded updated component config`.
- `no hierarchical-config pod restarted during the update`.
- `FULL HIERARCHICAL-CONFIG K8S E2E PASSED`.

Required verifier JSON evidence:

- top-level `"ok": true`.
- `ack.ok=true`.
- `ack.provenance.source="message"`.
- `ack.provenance.interface="update-catalog"`.
- `ack.provenance.volatile=true`.
- `initial.<token>.layerIds` contains the four scope layers plus the component layer.
- `initial.<token>.lineageMarker="k8s-hierarchical-initial"` for all four skeletons.
- `initial.<token>.publishInterval=5` for all four skeletons.
- `pushed.<token>.lineageMarker="k8s-hierarchical-updated"` for all four skeletons.
- `pushed.<token>.publishInterval=1` for all four skeletons.

The catalog keys and verifier request tokens are:

```text
JavaComponentSkeleton
PythonComponentSkeleton
RustComponentSkeleton
TsComponentSkeleton
```

The embedded standard config `component.token` values are:

```text
java-component-skeleton
python-component-skeleton
rust-component-skeleton
ts-component-skeleton
```

If the embedded tokens are CamelCase, the skeletons reject the effective config during schema
validation even though the ConfigComponent can still serve bundles.

### 6. Cleanup

If the run was executed without `KEEP=1`, the harness requests namespace cleanup on exit. If the run
used `KEEP=1`, clean up after collecting evidence:

```bash
kubectl delete namespace edgecommons-hierarchical --wait=false
```

It is normal for the namespace to remain in `Terminating` briefly after `--wait=false`.

### 7. Triage notes

If the VM shows little or no CPU while a run appears stuck, check the harness log and pid before
assuming the build is busy:

```bash
tail -220 /tmp/edgecommons-hierarchical-e2e.log
cat /tmp/edgecommons-hierarchical-e2e.status 2>/dev/null || true
kill -0 "$(cat /tmp/edgecommons-hierarchical-e2e.pid)" 2>/dev/null && echo running || echo stopped
kubectl -n edgecommons-hierarchical get pods 2>/dev/null || true
```

Common causes:

- the previous `KEEP=1` namespace is still terminating;
- a skeleton reached Kubernetes `Ready` but had not completed hierarchical bootstrap yet;
- Java logs `Dropping set-config push ... received before configuration bootstrap completed`, which
  means the update was sent too early;
- a skeleton rejects the served config because embedded `component.token` is not schema-valid;
- `kind load docker-image` failed, but direct `ctr` import succeeded. This is not a failure unless
  the fallback also fails.

## Evidence Checklist

Attach these to the validation note or PR:

- Greengrass deployment ids for:
  - ConfigComponent plus skeleton deployment.
  - Interop node deployment.
  - Hierarchical-config verifier deployment.
- `greengrass-cli component list` output showing all skeletons and ConfigComponent.
- ConfigComponent effective config showing volatile updates explicitly enabled for the test.
- Greengrass startup evidence showing each skeleton completed hierarchical bootstrap before the
  update was sent.
- Greengrass verifier JSON result, including acknowledgement provenance, `pushed_count:4`, and
  per-component lineage checks.
- Per-language skeleton reload log excerpts.
- File non-persistence proof for the Greengrass volatile catalog update.
- Kubernetes namespace pod list.
- Kubernetes harness log showing `FULL HIERARCHICAL-CONFIG K8S E2E PASSED`.
- Kubernetes verifier JSON result, including initial and pushed lineage bundles.
- Per-language Kubernetes reload log excerpts.
- ConfigMap non-persistence proof for the Kubernetes volatile catalog update when evidence is
  collected manually from a retained namespace.
- Any skipped item, with a clear reason and whether it blocks completion.
