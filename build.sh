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
# The on-device artifact is built with the `greengrass` feature (Greengrass IPC),
# which is Linux-only (the SDK is a C-FFI crate needing libclang). Build on a Linux
# host, or set GGCOMMONS_TARGET to a Linux triple you have a toolchain for, e.g.:
#   GGCOMMONS_TARGET=x86_64-unknown-linux-gnu ./build.sh
set -euo pipefail

# Keep these in sync with gdk-config.json.
COMPONENT_NAME="<<COMPONENTFULLNAME>>"
COMPONENT_VERSION="NEXT_PATCH"
BIN_NAME="<<BINNAME>>"

# Greengrass-mode features for the device build. Add ggcommons features here as
# needed, e.g. "greengrass,cloudwatch" for the direct CloudWatch metric target.
FEATURES="${GGCOMMONS_FEATURES:-greengrass}"
TARGET="${GGCOMMONS_TARGET:-}"

echo "Building ${BIN_NAME} (release, features=${FEATURES})${TARGET:+ for ${TARGET}}..."
if [[ -n "${TARGET}" ]]; then
  cargo build --release --no-default-features --features "${FEATURES}" --target "${TARGET}"
  BIN_DIR="target/${TARGET}/release"
else
  cargo build --release --no-default-features --features "${FEATURES}"
  BIN_DIR="target/release"
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
