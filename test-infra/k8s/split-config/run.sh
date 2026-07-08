#!/usr/bin/env bash
#
# Full split-config Kubernetes E2E:
#   EMQX + Rust ConfigComponent + Java/Python/Rust/TypeScript skeletons + verifier.
#
# Build and image-load are local to the current kind cluster; no registry push is required.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
K8S_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
UMBRELLA_ROOT="$(cd "${REPO_ROOT}/.." && pwd)"

NAMESPACE="${NAMESPACE:-edgecommons-split}"
CLUSTER_NAME="${CLUSTER_NAME:-edgecommons}"
TIMEOUT="${TIMEOUT:-300}"
KEEP="${KEEP:-}"

KUBECTL="${KUBECTL:-kubectl}"
KIND="${KIND:-kind}"
DOCKER="${DOCKER:-docker}"

log() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
ok() { printf '\033[1;32m  ok:\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31mFAIL:\033[0m %s\n' "$*" >&2; exit 1; }

cleanup() {
  local rc=$?
  if [[ $rc -ne 0 ]]; then
    log "FAILED (rc=${rc}) - diagnostics"
    "${KUBECTL}" -n "${NAMESPACE}" get pods,deploy,job,cm,svc 2>/dev/null || true
    "${KUBECTL}" -n "${NAMESPACE}" describe pods 2>/dev/null || true
    "${KUBECTL}" -n "${NAMESPACE}" logs deploy/edgecommons-config-component --tail=200 2>/dev/null || true
    "${KUBECTL}" -n "${NAMESPACE}" logs -l app.kubernetes.io/part-of=edgecommons-split-config --all-containers --tail=200 2>/dev/null || true
  fi
  if [[ "${KEEP}" != "1" ]]; then
    log "Cleaning namespace ${NAMESPACE}"
    "${KUBECTL}" delete namespace "${NAMESPACE}" --wait=false 2>/dev/null || true
  fi
}
trap cleanup EXIT

wait_for_log() {
  local selector="$1" pattern="$2" desc="$3" deadline=$((SECONDS + TIMEOUT)) out
  log "Asserting log: ${desc}"
  while (( SECONDS < deadline )); do
    out="$("${KUBECTL}" -n "${NAMESPACE}" logs -l "${selector}" --all-containers --tail=-1 2>/dev/null || true)"
    if grep -Eiq -- "${pattern}" <<<"${out}"; then
      ok "${desc}"
      return 0
    fi
    sleep 3
  done
  fail "timed out waiting for log match: ${desc} (/${pattern}/)"
}

ensure_cluster() {
  if "${KIND}" get clusters | grep -Fxq "${CLUSTER_NAME}"; then
    ok "kind cluster ${CLUSTER_NAME} already exists"
    return
  fi
  log "Creating kind cluster ${CLUSTER_NAME}"
  "${KIND}" create cluster --name "${CLUSTER_NAME}" --config "${K8S_DIR}/kind-config.yaml"
}

load_image() {
  local image="$1"
  log "Loading ${image} into kind/${CLUSTER_NAME}"
  if "${KIND}" load docker-image "${image}" --name "${CLUSTER_NAME}"; then
    ok "kind loaded ${image}"
    return
  fi
  log "kind load failed; importing ${image} directly into node containerd"
  "${DOCKER}" save "${image}" | "${DOCKER}" exec -i "${CLUSTER_NAME}-control-plane" ctr -n k8s.io images import -
  ok "containerd imported ${image}"
}

build_image() {
  local image="$1" dockerfile="$2" context="$3"
  log "Building ${image}"
  "${DOCKER}" build -f "${dockerfile}" -t "${image}" "${context}"
  load_image "${image}"
}

ensure_cluster

build_image "edgecommons-config-component:split-ci" "${UMBRELLA_ROOT}/config-component/Dockerfile" "${UMBRELLA_ROOT}"
build_image "edgecommons-java-skeleton:split-ci" "${SCRIPT_DIR}/Dockerfile.java-skeleton" "${REPO_ROOT}"
build_image "edgecommons-python-skeleton:split-ci" "${SCRIPT_DIR}/Dockerfile.python-skeleton" "${REPO_ROOT}"
build_image "edgecommons-rust-skeleton:split-ci" "${SCRIPT_DIR}/Dockerfile.rust-skeleton" "${REPO_ROOT}"
build_image "edgecommons-ts-skeleton:split-ci" "${SCRIPT_DIR}/Dockerfile.ts-skeleton" "${REPO_ROOT}"
build_image "edgecommons-split-verifier:split-ci" "${SCRIPT_DIR}/Dockerfile.verifier" "${REPO_ROOT}"

log "Preparing namespace ${NAMESPACE}"
if "${KUBECTL}" get namespace "${NAMESPACE}" >/dev/null 2>&1; then
  "${KUBECTL}" delete namespace "${NAMESPACE}" --wait=true --timeout="${TIMEOUT}s"
fi
"${KUBECTL}" create namespace "${NAMESPACE}" --dry-run=client -o yaml | "${KUBECTL}" apply -f -

log "Deploying EMQX"
"${KUBECTL}" -n "${NAMESPACE}" apply -f "${K8S_DIR}/emqx.yaml"
"${KUBECTL}" -n "${NAMESPACE}" rollout status deploy/edgecommons-emqx --timeout="${TIMEOUT}s"

log "Deploying ConfigComponent and skeletons"
"${KUBECTL}" -n "${NAMESPACE}" apply -f "${SCRIPT_DIR}/manifests.yaml"

for deploy in \
  edgecommons-config-component \
  edgecommons-java-skeleton \
  edgecommons-python-skeleton \
  edgecommons-rust-skeleton \
  edgecommons-ts-skeleton; do
  "${KUBECTL}" -n "${NAMESPACE}" rollout status "deploy/${deploy}" --timeout="${TIMEOUT}s"
done

wait_for_log "app.kubernetes.io/name=edgecommons-config-component" \
  "ConfigComponent subscribed.*get-configuration.*update-catalog|ConfigComponent subscribed" \
  "ConfigComponent subscribed to get-configuration and update-catalog"
wait_for_log "app.kubernetes.io/name=edgecommons-java-skeleton" \
  "Component initialization completed" \
  "Java skeleton completed initial split-config bootstrap"
wait_for_log "app.kubernetes.io/name=edgecommons-python-skeleton" \
  "EdgeCommons initialized successfully" \
  "Python skeleton completed initial split-config bootstrap"
wait_for_log "app.kubernetes.io/name=edgecommons-rust-skeleton" \
  "Rust Component Skeleton starting" \
  "Rust skeleton completed initial split-config bootstrap"
wait_for_log "app.kubernetes.io/name=edgecommons-ts-skeleton" \
  "TypeScript Component Skeleton starting" \
  "TypeScript skeleton completed initial split-config bootstrap"

log "Running split-config verifier job"
"${KUBECTL}" -n "${NAMESPACE}" delete job edgecommons-split-verifier --ignore-not-found
"${KUBECTL}" -n "${NAMESPACE}" apply -f "${SCRIPT_DIR}/verifier-job.yaml"
"${KUBECTL}" -n "${NAMESPACE}" wait --for=condition=complete job/edgecommons-split-verifier --timeout="${TIMEOUT}s"

VERIFIER_LOG="$("${KUBECTL}" -n "${NAMESPACE}" logs job/edgecommons-split-verifier)"
printf '%s\n' "${VERIFIER_LOG}"
grep -Fq '"ok": true' <<<"${VERIFIER_LOG}" || fail "verifier did not report ok=true"
ok "verifier proved initial get-configuration and update-catalog fanout"

wait_for_log "app.kubernetes.io/name=edgecommons-config-component" \
  "pushed catalog bundle.*k8s-split-updated|k8s-split-updated.*pushed catalog bundle" \
  "ConfigComponent pushed updated catalog bundles"
wait_for_log "app.kubernetes.io/name=edgecommons-java-skeleton" \
  "Configuration reload completed successfully|Publish interval changed from .* to 1000ms" \
  "Java skeleton dynamically reloaded updated component config"
wait_for_log "app.kubernetes.io/name=edgecommons-python-skeleton" \
  "set-config push received|Configuration change processed successfully|Notifying .*configuration change listeners" \
  "Python skeleton received set-config and notified listeners"
wait_for_log "app.kubernetes.io/name=edgecommons-rust-skeleton" \
  "configuration changed; updated publish interval to 1s|publish_interval.?=1" \
  "Rust skeleton dynamically reloaded updated component config"
wait_for_log "app.kubernetes.io/name=edgecommons-ts-skeleton" \
  "configuration changed; updated publish interval to 1s" \
  "TypeScript skeleton dynamically reloaded updated component config"

log "Checking pod restart counts"
restarts="$("${KUBECTL}" -n "${NAMESPACE}" get pods -l app.kubernetes.io/part-of=edgecommons-split-config -o jsonpath='{range .items[*]}{.metadata.name}{" "}{range .status.containerStatuses[*]}{.restartCount}{" "}{end}{"\n"}{end}')"
printf '%s\n' "${restarts}"
if awk '{ for (i=2; i<=NF; i++) if ($i != 0) exit 1 }' <<<"${restarts}"; then
  ok "no split-config pod restarted during the update"
else
  fail "one or more split-config pods restarted"
fi

log "FULL SPLIT-CONFIG K8S E2E PASSED"
