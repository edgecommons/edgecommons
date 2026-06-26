# Skeleton credential demos — local Greengrass deployments (lab-5950x)

Local (greengrass-cli) deployments created to verify `gg.credentials()` secret access in the
skeleton example components running as **deployed Greengrass components** (not just STANDALONE).
Each opens an encrypted file vault under its component work dir, seeds a demo secret on first run,
reads it back, and parses a typed basic-auth view — logging only non-sensitive facts (never the
value). Staged under `/tmp/ggdeploy/` on the lab.

| Component | Version | Final state | Notes |
|-----------|---------|-------------|-------|
| `com.mbreissi.greengrass.RustComponentSkeleton` | 1.0.0 | RUNNING | device binary built in WSL (`greengrass,credentials`); Run made RequiresPrivilege + self-chmod |
| `com.mbreissi.greengrass.JavaSkeletonCred` | 1.0.0 | RUNNING | distinct name to avoid the cloud-pinned `JavaComponentSkeleton 1.0.3` |
| `com.mbreissi.greengrass.TsComponentSkeleton` | 1.0.1 | RUNNING | built on the lab w/o the native streaming addon (stubbed); credentials use Node crypto |
| `com.mbreissi.greengrass.PythonComponentSkeleton` | 1.0.13 | RUNNING | ggc_user (NO RequiresPrivilege); needed 4 fixes — see below |

**All FOUR now run UNPRIVILEGED as ggc_user** (RequiresPrivilege removed from Java+TS recipes; Rust/Python never had it). Java needed a real fix first: a request/reply NullPointerException (late/duplicate reply → null future) that was non-fatal as root but, uncaught in the subscription handler under crash/restart churn, wedged the nucleus IPC. Fixed (commit 6ed774c: null-guard + subscription-callback try/catch). Nucleus was recovered with a user-authorized `systemctl restart greengrass`. Verified: Java runs as ggc_user, request/reply works, 0 NPEs, nucleus stable, cloud components healthy.

**On-device secret-access verified for ALL FOUR, all RUNNING** (logs: `Credentials vault initialized` → `seeded demo secret` → `credential access OK ... source=local` → `parsed basic-auth view username=svc-account`; value never logged).

**The "Python GG IPC bug" was a 4-layer onion, NOT an IPC bug** (GG IPC works for ggc_user). Fixed: (1) CloudWatch metric target crashed on an undefined measure → crash-restart loop → IPC connection storm (cloudwatch guard + skeleton measure name); (2) awscrt 0.34.1 slow connect >10s → IPC provider now retries w/ 30s timeout; (3) stale root-owned vault from earlier root experiments (cleaned); (4) python skeleton used the SHORT component name so {ComponentFullName} pointed at a non-ggc_user-writable path (now uses the full name, like Rust/Java/TS). Earlier Rust BROKEN was a lost exec-bit on GG re-stage (Run now self-chmods) — also not an IPC bug.

## Teardown (run on the lab — marc@192.168.1.229)
```bash
for c in RustComponentSkeleton JavaSkeletonCred PythonComponentSkeleton TsComponentSkeleton; do
  sudo /greengrass/v2/bin/greengrass-cli deployment create --remove "com.mbreissi.greengrass.$c"
done
rm -rf /tmp/ggdeploy
```
(The cloud-deployed `com.mbreissi.greengrass.JavaComponentSkeleton 1.0.3` is NOT ours — leave it.)

## Deploy specifics worth remembering
- greengrass-cli local deploy: `deployment create --recipeDir <r> --artifactDir <a> --merge Name=Version`.
- Recipes use gdk placeholders (`{COMPONENT_NAME}`/`{COMPONENT_VERSION}`, `s3://BUCKET_NAME/...`) that
  are only substituted at `gdk publish`; for a local deploy they must be replaced with concrete values.
- ZIP artifacts: GG extracts `<name>.zip` into a `<name>/` basename dir, so the zip's files must be at
  its **root** (no extra top-level dir), else paths double-nest.
- Python on this device needed `RequiresPrivilege: true` (root) for the GG IPC client to connect
  (as ggc_user the IPC connect timed out) **and** a system-wide `pip install --break-system-packages`
  (so the root Run sees the package). The wheel now declares cryptography+filelock (fixed in setup.py).
- TS streaming uses a native napi addon (`ggstreamlog-node`, needs node+cargo together — unavailable on
  the lab/WSL); the credentials demo doesn't use streaming, so it was stubbed and the streaming recipe
  section removed for this deploy.

## Parameters subsystem demo — added 2026-06-21 (gg.parameters())

The skeletons now also demonstrate `gg.parameters()` (offline-first externalized config) alongside
credentials. Source `env` (needs no AWS); on-device the env vars are set via the recipe `Run.Setenv`.

| Component | Version | State | Notes |
|-----------|---------|-------|-------|
| `com.mbreissi.greengrass.RustComponentSkeleton` | 1.0.2 | RUNNING (ggc_user) | built in WSL `greengrass,credentials,parameters`; parameter demo resolves /skeleton/region + /skeleton/poolSize from local cache (source=env, count=2); credentials still OK; 0 errors |
| `com.mbreissi.greengrass.PythonComponentSkeleton` | 1.0.14 | RUNNING (ggc_user) | new wheel (parameters module + config_manager wiring); `parameter access OK /skeleton/region=us-east-1`, `/skeleton/poolSize=8`, `source=env count=2`; credentials OK |
| `com.mbreissi.greengrass.TsComponentSkeleton` | 1.0.3 | RUNNING (ggc_user) | rebuilt lib+example dist on Win, shipped over lab's Linux node_modules; `parameter access OK: /skeleton/region=us-east-1`, typed poolSize=8, `source=env count=2`; credentials OK |
| `com.mbreissi.greengrass.JavaSkeletonCred` | 1.0.4 | RUNNING (ggc_user) | rebuilt JAR (lib 1.3.2-SNAPSHOT + example); `parameter access OK [param=/skeleton/region, value=us-east-1]`, poolSize=8, `ParameterStats[parameterCount=2, source=env, refreshFailures=0]`; credentials OK. 1.0.4 also carries the reply-subscription leak fix (see below). |

**ALL FOUR skeletons now RUNNING (ggc_user) with `gg.parameters()` resolving on-device** (source=env, count=2), credentials still working alongside. Recipes use `Run.Setenv` (GG_PARAM_SKELETON_REGION=us-east-1, GG_PARAM_SKELETON_POOLSIZE=8) + a `parameters` config section (env source, prefix GG_PARAM_, sync /skeleton/region + /skeleton/poolSize). Staged under /tmp/ggdeploy/{python,ts,java}/recipes2|recipes104 + artifacts/.../<version>.

### Java reply-subscription leak — found + fixed during this deploy (commit 742a07d)
While deploying Java parameters, the greengrass-cli went `Unable to create ipc client / TimeoutException` and new component startups stalled. Root cause was NOT message frequency (3s publish loop) but a **request/reply subscription leak**: the Java skeleton's `measureLatency()` (per-3s IOT_CORE latency probe) chained `.orTimeout().exceptionally()` but never cancelled the request on timeout. The library only auto-unsubscribes the `ggcommons/reply-<uuid>` topic on a *received* reply; with IoT Core unreachable the IOT_CORE probe timed out every cycle, orphaning ~150 subscriptions per 8-min rotation (subscribe-starts 300 vs unsub-stops 150). Over ~13h uptime this exhausted the IPC subscription quota. Fix: call `cancelRequest()`/`cancelRequestFromIoTCore()` on the timeout path (also hardened `publishRequest()`); mirrors the Python skeleton. **User-authorized `systemctl restart greengrass`** cleared the exhausted table; 1.0.4 then verified balanced (44 subscribe-starts = 44 unsub-stops in a clean 1-min window). Nucleus healthy afterward, 12 components RUNNING.

Parity note: Python already cancels on timeout. Worth checking whether the Rust/TS skeleton examples do an always-timing-out request/reply probe and, if so, that they cancel on timeout too.
