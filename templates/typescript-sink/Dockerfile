# Container image for a edgecommons TypeScript component, for the KUBERNETES (and HOST/Docker) platform.
# Requires @edgecommons/edgecommons resolvable from GitHub Packages npm (an .npmrc with the registry
# + a GITHUB_TOKEN at build time); see docs/platform/DESIGN-packaging.md §13.
#
# Multi-stage: stage 1 installs deps and compiles the TypeScript to dist/; stage 2 is a slim
# runtime carrying only dist/ + node_modules + package.json, run as a non-root user.
#
# Build from THIS directory (the npm install needs the GitHub Packages registry + a token), then
# load/push it and set `image:` in k8s/deployment.yaml:
#   docker build -t ghcr.io/<owner>/<component>:latest .
#   docker push ghcr.io/<owner>/<component>:latest      # or: kind load docker-image <image>

# ---- stage 1: build -------------------------------------------------------------------------
FROM node:20 AS build

WORKDIR /app

# Install deps first (better layer caching). package-lock.json is copied when present so `npm ci`
# can use it; tsconfig.json drives the tsc build that emits dist/.
COPY package.json tsconfig.json ./
COPY package-lock.json* ./
RUN npm ci

# Compile the TypeScript sources to dist/.
COPY src ./src
RUN npm run build

# ---- stage 2: runtime -----------------------------------------------------------------------
FROM node:20-slim AS runtime

WORKDIR /app

# Carry only the compiled output, installed deps, and the manifest.
COPY --from=build /app/dist          ./dist
COPY --from=build /app/node_modules  ./node_modules
COPY --from=build /app/package.json  ./package.json

# Run as a non-root, unprivileged user (matches the Deployment's runAsNonRoot securityContext).
USER 65532:65532

# No default args: with --platform auto the library auto-detects KUBERNETES (or HOST) and
# defaults its config source / transport / identity accordingly. Override via the Deployment's
# args: if you need an explicit platform/transport/config-source.
ENTRYPOINT ["node", "/app/dist/main.js"]
