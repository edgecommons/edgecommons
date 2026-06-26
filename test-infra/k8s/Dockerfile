# Component image for the Phase-1a Kubernetes smoke test.
#
# Builds a minimal PYTHON ggcommons component (the examples/python skeleton) on top of
# the local libs/python library, so the smoke test exercises the CONFIGMAP source on a
# real pod. The other languages have their own images; this is the default the k8s CI
# job builds + `kind load`s. Build from the REPO ROOT:
#
#   docker build -f test-infra/k8s/Dockerfile -t ggcommons-component:ci .
#
# The component auto-detects platform=KUBERNETES from the SA token and is launched with
#   -c CONFIGMAP /etc/ggcommons config.json
# (supplied by the Helm chart's `args`).
FROM python:3.12-slim

# Non-root runtime (matches the chart securityContext: runAsNonRoot + readOnlyRootFilesystem).
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

WORKDIR /app

# Install the library first (better layer caching), then the example deps.
COPY libs/python /src/libs/python
RUN pip install --no-cache-dir /src/libs/python

# The example component (skeleton) + its app package.
COPY examples/python/main.py        /app/main.py
COPY examples/python/app            /app/app
COPY examples/python/requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt || true

# Drop to a non-root UID; /tmp is provided writable by the chart (emptyDir).
USER 65532:65532

ENTRYPOINT ["python3", "/app/main.py"]
# Default args are supplied by the Helm chart (-c CONFIGMAP /etc/ggcommons config.json).
