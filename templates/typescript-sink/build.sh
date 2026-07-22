#!/usr/bin/env bash
#
# Custom GDK build for a TypeScript Greengrass component.
#
# `gdk component build` invokes this (see gdk-config.json -> custom_build_command).
# The GDK contract for a custom build system is that this script must place:
#   - the recipe   in  greengrass-build/recipes/
#   - the artifact in  greengrass-build/artifacts/<ComponentName>/<ComponentVersion>/
# (GDK creates those folders before calling us.)
#
# The artifact is a ZIP of the prebuilt component (dist/ + node_modules/ +
# package.json) so the on-device Run lifecycle just runs node against dist/main.js
# (no npm install on the core). node_modules are installed with --omit=dev so the
# bundle excludes devDependencies (typescript, @types/node).
set -euo pipefail

# Keep these in sync with gdk-config.json / recipe.yaml.
COMPONENT_NAME="<<COMPONENTFULLNAME>>"
COMPONENT_VERSION="1.0.0"
ARTIFACT_BASE="<<BINNAME>>"

echo "Installing dependencies (npm install)..."
npm install

echo "Compiling TypeScript (npm run build)..."
npm run build

echo "Pruning dev dependencies for the runtime bundle..."
npm install --omit=dev

# Stage the runtime files under a folder named after the artifact base so the
# unarchived path on-device is <decompressedPath>/<ARTIFACT_BASE>/dist/main.js.
STAGE_DIR="$(mktemp -d)"
PKG_DIR="${STAGE_DIR}/${ARTIFACT_BASE}"
mkdir -p "${PKG_DIR}"
cp -r dist "${PKG_DIR}/dist"
cp -r node_modules "${PKG_DIR}/node_modules"
cp package.json "${PKG_DIR}/package.json"

ARTIFACT_DIR="greengrass-build/artifacts/${COMPONENT_NAME}/${COMPONENT_VERSION}"
RECIPE_DIR="greengrass-build/recipes"
mkdir -p "${ARTIFACT_DIR}" "${RECIPE_DIR}"

ZIP_PATH="$(pwd)/${ARTIFACT_DIR}/${ARTIFACT_BASE}.zip"
rm -f "${ZIP_PATH}"
( cd "${STAGE_DIR}" && zip -r -q "${ZIP_PATH}" "${ARTIFACT_BASE}" )
rm -rf "${STAGE_DIR}"

# Restore dev dependencies for local development after the bundle is built.
npm install >/dev/null 2>&1 || true

cp recipe.yaml "${RECIPE_DIR}/recipe.yaml"

echo "Staged artifact -> ${ARTIFACT_DIR}/${ARTIFACT_BASE}.zip"
echo "Staged recipe   -> ${RECIPE_DIR}/recipe.yaml"
