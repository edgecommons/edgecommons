#!/usr/bin/env bash
#
# smoke.sh — Phase-1a Kubernetes smoke test for the ggcommons CONFIGMAP source.
#
# SCAFFOLD for the ORCHESTRATOR to run LIVE against a real cluster (kind or lab k3s).
# It installs the in-cluster EMQX broker + the component chart, waits for Ready, then
# asserts:
#   1. the pod resolved platform=KUBERNETES (auto-detected from the SA token),
#   2. it loaded config via the CONFIGMAP source,
#   3. it connected to the in-cluster broker by Service DNS,
#   4. a `kubectl patch` of the ConfigMap is HOT-RELOADED in-process (the ..data
#      atomic-swap re-arm test) — no pod restart.
#
# This script does NOT build or load the image and is NOT run by the library unit
# tests; the CI workflow (.github/workflows/k8s.yml) builds+loads the image first.
#
# Usage:
#   IMAGE=ggcommons-component:ci NAMESPACE=ggcommons ./smoke.sh
# Env knobs (all optional; defaults shown):
#   IMAGE        component image ref (repo:tag) already loaded into the cluster
#   NAMESPACE    namespace to deploy into                       (ggcommons)
#   RELEASE      helm release name                              (ggc)
#   TIMEOUT      per-wait timeout, seconds                      (180)
#   KEEP         if "1", do not delete the namespace on exit    (unset)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART_DIR="${SCRIPT_DIR}/chart"

IMAGE="${IMAGE:-ggcommons-component:ci}"
NAMESPACE="${NAMESPACE:-ggcommons}"
RELEASE="${RELEASE:-ggc}"
TIMEOUT="${TIMEOUT:-180}"
HELM="${HELM:-helm}"
KUBECTL="${KUBECTL:-kubectl}"

IMAGE_REPO="${IMAGE%:*}"
IMAGE_TAG="${IMAGE##*:}"
SELECTOR="app.kubernetes.io/instance=${RELEASE}"

log()  { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m  ok:\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31mFAIL:\033[0m %s\n' "$*" >&2; exit 1; }

cleanup() {
  local rc=$?
  if [[ $rc -ne 0 ]]; then
    log "FAILED (rc=$rc) — dumping diagnostics"
    "${KUBECTL}" -n "${NAMESPACE}" get pods,svc,cm 2>/dev/null || true
    "${KUBECTL}" -n "${NAMESPACE}" logs -l "${SELECTOR}" --tail=200 2>/dev/null || true
  fi
  if [[ "${KEEP:-}" != "1" ]]; then
    log "Cleaning up namespace ${NAMESPACE}"
    "${HELM}" -n "${NAMESPACE}" uninstall "${RELEASE}" 2>/dev/null || true
    "${KUBECTL}" delete -n "${NAMESPACE}" -f "${SCRIPT_DIR}/emqx.yaml" 2>/dev/null || true
    "${KUBECTL}" delete namespace "${NAMESPACE}" --wait=false 2>/dev/null || true
  fi
}
trap cleanup EXIT

# Wait until `kubectl logs` for the release contains $1 (extended regex), or fail.
#
# NB: we CAPTURE the logs into a variable and grep a here-string rather than piping
# `kubectl logs ... | grep -q`. Under `set -o pipefail`, `grep -q` exits on the first match
# and closes the pipe, so `kubectl logs` (still streaming the rest) dies with SIGPIPE (141) —
# making the pipeline status non-zero EVEN ON A MATCH, so the `if` would never fire and the
# assertion would time out (observed on a busy k3s with kubectl 1.35.x). The capture form has
# no pipe to break; `|| true` neutralizes any kubectl non-zero exit so only the grep decides.
assert_log() {
  local pattern="$1" desc="$2" deadline=$((SECONDS + TIMEOUT)) out
  log "Asserting log: ${desc}"
  while (( SECONDS < deadline )); do
    out="$("${KUBECTL}" -n "${NAMESPACE}" logs -l "${SELECTOR}" --tail=-1 2>/dev/null || true)"
    if grep -Eiq -- "${pattern}" <<<"${out}"; then
      ok "${desc}"
      return 0
    fi
    sleep 3
  done
  fail "timed out waiting for log match: ${desc} (/${pattern}/)"
}

# ----------------------------------------------------------------------------------
log "Namespace ${NAMESPACE}"
"${KUBECTL}" create namespace "${NAMESPACE}" --dry-run=client -o yaml | "${KUBECTL}" apply -f -

log "Deploying in-cluster EMQX broker"
"${KUBECTL}" apply -n "${NAMESPACE}" -f "${SCRIPT_DIR}/emqx.yaml"
"${KUBECTL}" -n "${NAMESPACE}" rollout status deploy/ggcommons-emqx --timeout="${TIMEOUT}s"

log "Installing the component chart (image ${IMAGE})"
"${HELM}" upgrade --install "${RELEASE}" "${CHART_DIR}" \
  --namespace "${NAMESPACE}" \
  --set image.repository="${IMAGE_REPO}" \
  --set image.tag="${IMAGE_TAG}" \
  --set image.pullPolicy=IfNotPresent \
  --wait --timeout "${TIMEOUT}s"

log "Waiting for the component rollout"
"${KUBECTL}" -n "${NAMESPACE}" rollout status deploy/"${RELEASE}"-ggcommons-component --timeout="${TIMEOUT}s"

# --- Core assertions: CONFIGMAP source + broker connect (FR-MSG-1) + identity (FR-RT-7) -------
# NOTE: the resolver's "Resolved platform=KUBERNETES" and messaging's "Successfully connected" logs
# are emitted BEFORE the component configures logging (both precede config load), so they are dropped
# and not assertable from pod logs. The stdout-JSON sink (1c-logging) is now in place, but the
# early-logging-bootstrap that would make those pre-config startup lines visible is still deferred, so
# we keep asserting on reliably-emitted logs. (Human-readable messages still match here because they
# appear verbatim as the "message" field inside each JSON log line.)
#
# FR-MSG-1: the chart `args` pass NO positional `--transport MQTT <path>` (and no --transport at all);
# the KUBERNETES profile derives transport=MQTT and the messaging-config path DEFAULTS to the mounted
# ConfigMap file. So a successful in-cluster broker round-trip proves the broker config was sourced
# from the ConfigMap with no positional path.
assert_log "Starting ConfigMap directory watcher on /etc/ggcommons" "config loaded via the CONFIGMAP source (KUBERNETES profile)"
assert_log "Received an .* message on topic ggcommons" "MQTT round-trip via in-cluster broker — broker config from ConfigMap, no positional path (FR-MSG-1)"

# FR-RT-7: no -t/--thing is passed, so the resolved identity must come from the Downward-API env. With
# `thingName` unset, GGCOMMONS_THING_NAME is absent and identity falls through to POD_NAME (the pod's
# metadata.name, injected via a Downward-API fieldRef). The skeleton logs its resolved identity once at
# startup; assert it equals this pod's actual name.
POD="$("${KUBECTL}" -n "${NAMESPACE}" get pods -l "${SELECTOR}" -o jsonpath='{.items[0].metadata.name}')"
[[ -n "${POD}" ]] || fail "could not determine the component pod name"
assert_log "Component identity .thing name.: ${POD}" "Downward-API identity resolved to POD_NAME=${POD} (FR-RT-7)"

# FR-LOG-1/3: on KUBERNETES the default logging sink is structured stdout-JSON (one JSON object per
# line), and each line carries Downward-API correlation fields. Assert a JSON line whose `thing`
# correlation equals this pod's POD_NAME — this proves the json sink is the k8s default AND that the
# correlation fields are wired (one assertion covers both). (json.dumps emits `: ` with a space.)
assert_log "\"thing\": *\"${POD}\"" "stdout-JSON logging sink with Downward-API correlation (FR-LOG-1/3)"

# --- FR-HB-1: HTTP health endpoint -------------------------------------------------
# The chart wires httpGet startup/liveness/readiness probes to the component's health server
# (/startupz, /livez, /readyz on :8081 — the KUBERNETES default, no `health` config needed). The
# rollout above already gates on Ready, so /startupz and /readyz must have returned 200. Assert the
# Ready condition explicitly, then do an INDEPENDENT in-pod GET of /livez to prove liveness is served
# and decoupled from the broker. (restartCount==0, asserted below, also confirms /livez isn't failing.)
log "Asserting HTTP health endpoint (/livez, /readyz, /startupz)"
READY="$("${KUBECTL}" -n "${NAMESPACE}" get pods -l "${SELECTOR}" -o jsonpath='{.items[0].status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
[[ "${READY}" == "True" ]] || fail "pod is not Ready — the httpGet /readyz (and /startupz) probe did not return 200"
ok "readiness probe /readyz + startup probe /startupz returned 200 (pod Ready) (FR-HB-1)"
if "${KUBECTL}" -n "${NAMESPACE}" exec "${POD}" -- python3 -c "import urllib.request,sys; sys.exit(0 if urllib.request.urlopen('http://127.0.0.1:8081/livez',timeout=5).getcode()==200 else 1)" >/dev/null 2>&1; then
  ok "liveness probe /livez returned 200 from inside the pod (FR-HB-1)"
else
  fail "/livez did not return 200 from inside the pod"
fi

# --- FR-MET-1: prometheus metrics target -------------------------------------------
# On KUBERNETES the metric target DEFAULTS to `prometheus` (no metricEmission.target in the config),
# so the component serves an in-process registry as OpenMetrics text at :9090/metrics. The heartbeat
# (routed to the metric target) populates it within an interval. Poll an in-pod GET of /metrics until a
# ggcommons-namespaced gauge appears (proves: pull endpoint up + valid exposition + heartbeat scraped).
log "Asserting prometheus /metrics endpoint (FR-MET-1)"
metrics_ok=""
for _ in $(seq 1 20); do
  if "${KUBECTL}" -n "${NAMESPACE}" exec "${POD}" -- python3 -c "
import urllib.request,sys
r=urllib.request.urlopen('http://127.0.0.1:9090/metrics',timeout=5)
b=r.read().decode()
sys.exit(0 if r.getcode()==200 and '# TYPE' in b and 'ggcommons_' in b else 1)
" >/dev/null 2>&1; then
    metrics_ok=1
    break
  fi
  sleep 3
done
[[ -n "${metrics_ok}" ]] || fail "/metrics did not serve a ggcommons_* gauge (prometheus target / heartbeat)"
ok "prometheus /metrics serves OpenMetrics text with a ggcommons_* gauge (FR-MET-1)"

# --- Hot-reload (..data swap re-arm) test -------------------------------------------
# Patch the ConfigMap's config.json in place (NOT helm upgrade) and assert the running
# pod reloads in-process. We flip logging.level INFO->DEBUG as the observable change.
log "Patching the ConfigMap to trigger the ..data hot-reload"
# Portable interpreter: python3 on Linux/CI, python on Windows Git Bash.
PY="$(command -v python3 || command -v python || true)"
[[ -n "${PY}" ]] || fail "need python3/python to JSON-encode the ConfigMap patch payload"
CM_NAME="${RELEASE}-ggcommons-component-config"
CURRENT_JSON="$("${KUBECTL}" -n "${NAMESPACE}" get configmap "${CM_NAME}" -o jsonpath='{.data.config\.json}')"
NEW_JSON="$(printf '%s' "${CURRENT_JSON}" | sed 's/"level": *"INFO"/"level": "DEBUG"/')"
if [[ "${NEW_JSON}" == "${CURRENT_JSON}" ]]; then
  fail "could not mutate config.json (logging.level INFO not found) — check the rendered ConfigMap"
fi
# Replace the whole data key via a strategic-merge patch (JSON-encode the doc as a string).
"${KUBECTL}" -n "${NAMESPACE}" patch configmap "${CM_NAME}" --type merge \
  -p "$(printf '{"data":{"config.json":%s}}' "$(printf '%s' "${NEW_JSON}" | "${PY}" -c 'import json,sys; print(json.dumps(sys.stdin.read()))')")"

# kubelet propagation of a ConfigMap edit to a mounted volume can be as long as the kubelet
# sync period + the ConfigMap cache TTL (~1m + ~1m at defaults), and is slower still on loaded
# CI runners. Allow generous time; override with RELOAD_TIMEOUT (CI sets it higher — see k8s.yml).
RELOAD_TIMEOUT="${RELOAD_TIMEOUT:-240}"
TIMEOUT="${RELOAD_TIMEOUT}" assert_log \
  "ConfigMap changed|configuration reloaded|reloaded config|config.*reload" \
  "in-process hot-reload after the ..data swap (no restart)"

# Confirm the pod did NOT restart (hot-reload, not a roll).
RESTARTS="$("${KUBECTL}" -n "${NAMESPACE}" get pods -l "${SELECTOR}" \
  -o jsonpath='{.items[*].status.containerStatuses[*].restartCount}')"
for r in ${RESTARTS}; do
  [[ "${r}" == "0" ]] || fail "pod restarted (restartCount=${r}) — expected in-process reload, not a roll"
done
ok "pod did not restart (restartCount=0) — reload was in-process"

log "SMOKE TEST PASSED"
