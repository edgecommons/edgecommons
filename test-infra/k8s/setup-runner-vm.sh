#!/usr/bin/env bash
#
# setup-runner-vm.sh
#
# Bootstrap a dedicated Ubuntu VM to run repeatable EdgeCommons Kubernetes E2E
# tests. Run on the VM itself, not inside the kind control-plane container.
#
# Defaults are pinned where upstream provides stable binary releases. For tools
# with an LTS channel, defaults track the current LTS line as of this script's
# maintenance date: Java 25 LTS and Node.js 24 LTS. Docker CE does not publish
# an LTS channel; the default pins the current Docker stable package expected
# for the supported Ubuntu repos and fails with the available versions if the
# selected Ubuntu repo does not publish that exact build.
#
# Usage:
#   bash test-infra/k8s/setup-runner-vm.sh
#
# Common overrides:
#   RUN_PROBE=0 bash test-infra/k8s/setup-runner-vm.sh
#   DOCKER_ENGINE_VERSION=29.6.1 bash test-infra/k8s/setup-runner-vm.sh
#   DOCKER_APT_VERSION='5:29.6.1-1~ubuntu.26.04~resolute' bash ...

set -Eeuo pipefail

DOCKER_ENGINE_VERSION="${DOCKER_ENGINE_VERSION:-29.6.1}"
DOCKER_COMPOSE_VERSION="${DOCKER_COMPOSE_VERSION:-v2.38.2}"
DOCKER_BUILDX_VERSION="${DOCKER_BUILDX_VERSION:-v0.26.1}"
KUBECTL_VERSION="${KUBECTL_VERSION:-v1.36.1}"
KIND_VERSION="${KIND_VERSION:-v0.30.0}"
KIND_NODE_IMAGE="${KIND_NODE_IMAGE:-kindest/node:v1.36.1}"
HELM_VERSION="${HELM_VERSION:-v3.19.3}"
PROMETHEUS_STACK_CHART_VERSION="${PROMETHEUS_STACK_CHART_VERSION:-87.10.1}"
SETUP_HELM_REPOS="${SETUP_HELM_REPOS:-1}"
YQ_VERSION="${YQ_VERSION:-v4.45.4}"
JDK_VERSION="${JDK_VERSION:-25}"
NODE_VERSION="${NODE_VERSION:-v24.18.0}"
MAVEN_VERSION="${MAVEN_VERSION:-3.9.11}"
RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-1.96.0}"
RUN_PROBE="${RUN_PROBE:-1}"
PROBE_CLUSTER_NAME="${PROBE_CLUSTER_NAME:-edgecommons-prereq-probe}"

log() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
ok() { printf '\033[1;32m  ok:\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mWARN:\033[0m %s\n' "$*" >&2; }
fail() { printf '\033[1;31mFAIL:\033[0m %s\n' "$*" >&2; exit 1; }

if [[ "$(id -u)" -eq 0 ]]; then
  SUDO=""
  TARGET_USER="${SUDO_USER:-root}"
else
  command -v sudo >/dev/null 2>&1 || fail "sudo is required when not running as root"
  SUDO="sudo"
  TARGET_USER="${USER}"
fi

if [[ ! -r /etc/os-release ]]; then
  fail "cannot identify OS: /etc/os-release is missing"
fi
# shellcheck disable=SC1091
. /etc/os-release

[[ "${ID:-}" == "ubuntu" ]] || fail "this bootstrap supports Ubuntu only; found ID=${ID:-unknown}"
[[ -n "${VERSION_ID:-}" ]] || fail "VERSION_ID missing from /etc/os-release"
[[ -n "${VERSION_CODENAME:-}" ]] || fail "VERSION_CODENAME missing from /etc/os-release"

case "$(uname -m)" in
  x86_64)
    DEB_ARCH="amd64"
    GO_ARCH="amd64"
    NODE_ARCH="x64"
    COMPOSE_ARCH="x86_64"
    ;;
  aarch64 | arm64)
    DEB_ARCH="arm64"
    GO_ARCH="arm64"
    NODE_ARCH="arm64"
    COMPOSE_ARCH="aarch64"
    ;;
  *)
    fail "unsupported CPU architecture: $(uname -m)"
    ;;
esac

DOCKER_APT_VERSION="${DOCKER_APT_VERSION:-5:${DOCKER_ENGINE_VERSION}-1~ubuntu.${VERSION_ID}~${VERSION_CODENAME}}"

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

download() {
  local url="$1" out="$2"
  curl --fail --location --show-error --silent --retry 5 --retry-delay 2 -o "${out}" "${url}"
}

install_executable() {
  local source="$1" target="$2"
  ${SUDO} install -m 0755 "${source}" "${target}"
}

run_as_target_user() {
  if [[ "${TARGET_USER}" == "root" ]]; then
    bash -lc "$*"
  else
    ${SUDO} -H -u "${TARGET_USER}" bash -lc "$*"
  fi
}

log "Installing base OS packages"
${SUDO} apt-get update
${SUDO} DEBIAN_FRONTEND=noninteractive apt-get install -y \
  apt-transport-https \
  bash-completion \
  build-essential \
  ca-certificates \
  clang \
  cmake \
  curl \
  git \
  gnupg \
  jq \
  libssl-dev \
  lsb-release \
  make \
  ninja-build \
  pkg-config \
  protobuf-compiler \
  python3 \
  python3-pip \
  python3-venv \
  tar \
  unzip \
  xz-utils \
  zip

log "Configuring Eclipse Adoptium apt repository"
download "https://packages.adoptium.net/artifactory/api/gpg/key/public" "${TMP_DIR}/adoptium.asc"
gpg --batch --yes --dearmor -o "${TMP_DIR}/adoptium.gpg" "${TMP_DIR}/adoptium.asc"
${SUDO} install -m 0644 "${TMP_DIR}/adoptium.gpg" /etc/apt/trusted.gpg.d/adoptium.gpg
printf 'deb https://packages.adoptium.net/artifactory/deb %s main\n' "${VERSION_CODENAME}" |
  ${SUDO} tee /etc/apt/sources.list.d/adoptium.list >/dev/null
${SUDO} apt-get update

log "Installing Eclipse Temurin ${JDK_VERSION} JDK"
${SUDO} DEBIAN_FRONTEND=noninteractive apt-get install -y "temurin-${JDK_VERSION}-jdk"
${SUDO} apt-mark hold "temurin-${JDK_VERSION}-jdk" >/dev/null

log "Configuring Docker CE apt repository"
${SUDO} install -m 0755 -d /etc/apt/keyrings
download "https://download.docker.com/linux/ubuntu/gpg" "${TMP_DIR}/docker.asc"
${SUDO} install -m 0644 "${TMP_DIR}/docker.asc" /etc/apt/keyrings/docker.asc
${SUDO} chmod a+r /etc/apt/keyrings/docker.asc
printf 'deb [arch=%s signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu %s stable\n' \
  "${DEB_ARCH}" "${VERSION_CODENAME}" |
  ${SUDO} tee /etc/apt/sources.list.d/docker.list >/dev/null
${SUDO} apt-get update

if ! apt-cache madison docker-ce | awk '{print $3}' | grep -Fx "${DOCKER_APT_VERSION}" >/dev/null; then
  warn "Pinned Docker package not found: ${DOCKER_APT_VERSION}"
  warn "Available docker-ce versions:"
  apt-cache madison docker-ce | sed 's/^/  /' >&2 || true
  fail "set DOCKER_APT_VERSION to one of the available package versions, or choose an Ubuntu codename supported by Docker"
fi

log "Installing Docker CE ${DOCKER_APT_VERSION}"
${SUDO} DEBIAN_FRONTEND=noninteractive apt-get install -y \
  "docker-ce=${DOCKER_APT_VERSION}" \
  "docker-ce-cli=${DOCKER_APT_VERSION}" \
  "docker-ce-rootless-extras=${DOCKER_APT_VERSION}" \
  containerd.io
${SUDO} apt-mark hold docker-ce docker-ce-cli docker-ce-rootless-extras containerd.io >/dev/null
${SUDO} systemctl enable --now docker
if getent group docker >/dev/null && [[ "${TARGET_USER}" != "root" ]]; then
  ${SUDO} usermod -aG docker "${TARGET_USER}"
  warn "User ${TARGET_USER} was added to the docker group; log out/in after this script for group membership to refresh"
fi

log "Installing Docker CLI plugins"
${SUDO} install -m 0755 -d /usr/local/lib/docker/cli-plugins
download "https://github.com/docker/compose/releases/download/${DOCKER_COMPOSE_VERSION}/docker-compose-linux-${COMPOSE_ARCH}" "${TMP_DIR}/docker-compose"
install_executable "${TMP_DIR}/docker-compose" /usr/local/lib/docker/cli-plugins/docker-compose
download "https://github.com/docker/buildx/releases/download/${DOCKER_BUILDX_VERSION}/buildx-${DOCKER_BUILDX_VERSION}.linux-${GO_ARCH}" "${TMP_DIR}/docker-buildx"
install_executable "${TMP_DIR}/docker-buildx" /usr/local/lib/docker/cli-plugins/docker-buildx

log "Installing kubectl ${KUBECTL_VERSION}"
download "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${GO_ARCH}/kubectl" "${TMP_DIR}/kubectl"
download "https://dl.k8s.io/release/${KUBECTL_VERSION}/bin/linux/${GO_ARCH}/kubectl.sha256" "${TMP_DIR}/kubectl.sha256"
printf '%s  %s\n' "$(cat "${TMP_DIR}/kubectl.sha256")" "${TMP_DIR}/kubectl" | sha256sum --check --status
install_executable "${TMP_DIR}/kubectl" /usr/local/bin/kubectl

log "Installing kind ${KIND_VERSION}"
download "https://kind.sigs.k8s.io/dl/${KIND_VERSION}/kind-linux-${GO_ARCH}" "${TMP_DIR}/kind"
if download "https://kind.sigs.k8s.io/dl/${KIND_VERSION}/kind-linux-${GO_ARCH}.sha256sum" "${TMP_DIR}/kind.sha256sum"; then
  printf '%s  %s\n' "$(awk '{print $1}' "${TMP_DIR}/kind.sha256sum")" "${TMP_DIR}/kind" | sha256sum --check --status
else
  warn "No kind checksum asset found; installed binary without checksum verification"
fi
install_executable "${TMP_DIR}/kind" /usr/local/bin/kind

log "Installing Helm ${HELM_VERSION}"
download "https://get.helm.sh/helm-${HELM_VERSION}-linux-${GO_ARCH}.tar.gz" "${TMP_DIR}/helm.tar.gz"
if download "https://get.helm.sh/helm-${HELM_VERSION}-linux-${GO_ARCH}.tar.gz.sha256sum" "${TMP_DIR}/helm.sha256sum"; then
  printf '%s  %s\n' "$(awk '{print $1}' "${TMP_DIR}/helm.sha256sum")" "${TMP_DIR}/helm.tar.gz" | sha256sum --check --status
else
  warn "No Helm checksum asset found; installed binary without checksum verification"
fi
tar -xzf "${TMP_DIR}/helm.tar.gz" -C "${TMP_DIR}"
install_executable "${TMP_DIR}/linux-${GO_ARCH}/helm" /usr/local/bin/helm

log "Installing yq ${YQ_VERSION}"
download "https://github.com/mikefarah/yq/releases/download/${YQ_VERSION}/yq_linux_${GO_ARCH}" "${TMP_DIR}/yq"
install_executable "${TMP_DIR}/yq" /usr/local/bin/yq

if [[ "${SETUP_HELM_REPOS}" == "1" ]]; then
  log "Configuring Helm repos for Kubernetes E2E add-ons"
  run_as_target_user "helm repo add prometheus-community https://prometheus-community.github.io/helm-charts --force-update"
  run_as_target_user "helm repo update prometheus-community"
else
  warn "SETUP_HELM_REPOS=0; skipped Helm repo configuration"
fi

log "Installing Node.js ${NODE_VERSION}"
NODE_BASENAME="node-${NODE_VERSION}-linux-${NODE_ARCH}"
download "https://nodejs.org/dist/${NODE_VERSION}/${NODE_BASENAME}.tar.xz" "${TMP_DIR}/${NODE_BASENAME}.tar.xz"
download "https://nodejs.org/dist/${NODE_VERSION}/SHASUMS256.txt" "${TMP_DIR}/node-shasums.txt"
(cd "${TMP_DIR}" && grep " ${NODE_BASENAME}.tar.xz\$" node-shasums.txt | sha256sum --check --status)
${SUDO} rm -rf "/opt/${NODE_BASENAME}"
${SUDO} tar -xJf "${TMP_DIR}/${NODE_BASENAME}.tar.xz" -C /opt
for bin in node npm npx corepack; do
  ${SUDO} ln -sfn "/opt/${NODE_BASENAME}/bin/${bin}" "/usr/local/bin/${bin}"
done

log "Installing Apache Maven ${MAVEN_VERSION}"
MAVEN_BASENAME="apache-maven-${MAVEN_VERSION}"
download "https://archive.apache.org/dist/maven/maven-3/${MAVEN_VERSION}/binaries/${MAVEN_BASENAME}-bin.tar.gz" "${TMP_DIR}/${MAVEN_BASENAME}-bin.tar.gz"
download "https://archive.apache.org/dist/maven/maven-3/${MAVEN_VERSION}/binaries/${MAVEN_BASENAME}-bin.tar.gz.sha512" "${TMP_DIR}/${MAVEN_BASENAME}-bin.tar.gz.sha512"
printf '%s  %s\n' "$(awk '{print $1}' "${TMP_DIR}/${MAVEN_BASENAME}-bin.tar.gz.sha512")" "${TMP_DIR}/${MAVEN_BASENAME}-bin.tar.gz" | sha512sum --check --status
${SUDO} rm -rf "/opt/${MAVEN_BASENAME}"
${SUDO} tar -xzf "${TMP_DIR}/${MAVEN_BASENAME}-bin.tar.gz" -C /opt
${SUDO} ln -sfn "/opt/${MAVEN_BASENAME}/bin/mvn" /usr/local/bin/mvn

log "Installing Rust toolchain ${RUST_TOOLCHAIN} for ${TARGET_USER}"
run_as_target_user "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain '${RUST_TOOLCHAIN}'"
run_as_target_user "source ~/.cargo/env && rustup component add rustfmt clippy"

log "Pre-pulling kind node image ${KIND_NODE_IMAGE}"
${SUDO} docker pull "${KIND_NODE_IMAGE}"

log "Installed tool versions"
docker --version || ${SUDO} docker --version
docker compose version || ${SUDO} docker compose version
docker buildx version || ${SUDO} docker buildx version
kubectl version --client=true
kind version
helm version --short
if [[ "${SETUP_HELM_REPOS}" == "1" ]]; then
  run_as_target_user "helm search repo prometheus-community/kube-prometheus-stack --version '${PROMETHEUS_STACK_CHART_VERSION}'"
fi
yq --version
node --version
npm --version
java -version
javac -version
mvn --version | head -5
python3 --version
run_as_target_user "source ~/.cargo/env && rustc --version && cargo --version"

if [[ "${RUN_PROBE}" == "1" ]]; then
  log "Running kind probe cluster ${PROBE_CLUSTER_NAME}"
  if ${SUDO} kind get clusters | grep -Fx "${PROBE_CLUSTER_NAME}" >/dev/null 2>&1; then
    warn "Deleting existing probe cluster ${PROBE_CLUSTER_NAME}"
    ${SUDO} kind delete cluster --name "${PROBE_CLUSTER_NAME}"
  fi
  ${SUDO} kind create cluster --name "${PROBE_CLUSTER_NAME}" --image "${KIND_NODE_IMAGE}" --wait 180s
  ${SUDO} kubectl --context "kind-${PROBE_CLUSTER_NAME}" get nodes -o wide
  helm version --short >/dev/null
  ${SUDO} kind delete cluster --name "${PROBE_CLUSTER_NAME}"
  ok "kind probe cluster succeeded and was deleted"
else
  warn "RUN_PROBE=0; skipped kind cluster probe"
fi

ok "Kubernetes E2E runner VM prerequisites installed"
warn "If docker was newly installed, log out and back in before running tests without sudo"
