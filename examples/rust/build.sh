#!/usr/bin/env bash
#
# Custom GDK build for a Rust Greengrass component.
#
# `gdk component build` invokes this (see gdk-config.json -> custom_build_command).
# The GDK contract for a custom build system is that this script must place:
#   - the recipe   in  greengrass-build/recipes/
#   - the artifact in  greengrass-build/artifacts/<ComponentName>/<ComponentVersion>/
# (GDK creates those folders before calling us.)
#
# Cross-compilation note: Greengrass cores typically run Linux. Build on a Linux
# host, or set GGCOMMONS_TARGET to a Linux triple you have a toolchain for, e.g.:
#   GGCOMMONS_TARGET=x86_64-unknown-linux-gnu ./build.sh
set -euo pipefail

# Keep these in sync with gdk-config.json / recipe.yaml.
COMPONENT_NAME="aws.proserve.greengrass.RustComponentSkeleton"
COMPONENT_VERSION="1.0.0"
BIN_NAME="rust-component-skeleton"

# Device artifact uses the Greengrass IPC feature (Linux-only; needs libclang).
# To also ship durable telemetry streaming to Kinesis (recipe `streaming` section),
# build with the streaming sink feature, e.g.:
#   GGCOMMONS_FEATURES="greengrass,streaming-kinesis" ./build.sh
# (Requires the target Kinesis stream to exist + TES role kinesis:PutRecords.)
FEATURES="${GGCOMMONS_FEATURES:-greengrass}"
TARGET="${GGCOMMONS_TARGET:-}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"

echo "Building ${BIN_NAME} (release, features=${FEATURES})${TARGET:+ for ${TARGET}}..."
if [[ -n "${TARGET}" ]]; then
  cargo build --release --no-default-features --features "${FEATURES}" --target "${TARGET}"
  BIN_DIR="${TARGET_DIR}/${TARGET}/release"
else
  cargo build --release --no-default-features --features "${FEATURES}"
  BIN_DIR="${TARGET_DIR}/release"
fi

# Resolve the binary path (Windows host builds produce a .exe).
BIN_PATH="${BIN_DIR}/${BIN_NAME}"
[[ -f "${BIN_PATH}" ]] || BIN_PATH="${BIN_DIR}/${BIN_NAME}.exe"
if [[ ! -f "${BIN_PATH}" ]]; then
  echo "error: built binary not found in ${BIN_DIR}" >&2
  exit 1
fi

ARTIFACT_DIR="greengrass-build/artifacts/${COMPONENT_NAME}/${COMPONENT_VERSION}"
RECIPE_DIR="greengrass-build/recipes"
mkdir -p "${ARTIFACT_DIR}" "${RECIPE_DIR}"

cp "${BIN_PATH}" "${ARTIFACT_DIR}/${BIN_NAME}"
chmod +x "${ARTIFACT_DIR}/${BIN_NAME}" || true
cp recipe.yaml "${RECIPE_DIR}/recipe.yaml"

echo "Staged artifact -> ${ARTIFACT_DIR}/${BIN_NAME}"
echo "Staged recipe   -> ${RECIPE_DIR}/recipe.yaml"
