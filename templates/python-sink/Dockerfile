# Container image for the <<COMPONENTNAME>> edgecommons component on Kubernetes.
#
# Requires the edgecommons Python library to be resolvable (PyPI or a pip git+https dep);
# see docs/platform/DESIGN-packaging.md §13. This is a STANDALONE scaffold: unlike the
# in-monorepo image it does NOT copy a local libs/ tree — the library is installed from
# the registry via requirements.txt (which lists `edgecommons`).
#
# Build from THIS directory and load/push it, then set `image:` in k8s/deployment.yaml:
#   docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
#   docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest      # or: kind load docker-image ...
FROM python:3.12-slim

# Non-root runtime (matches k8s/deployment.yaml securityContext: runAsNonRoot + readOnlyRootFilesystem).
ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

WORKDIR /app

# Install dependencies first (better layer caching). requirements.txt lists `edgecommons`,
# resolved from the registry (PyPI or a pip git+https dep) — no monorepo checkout needed.
COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt

# The component entry point + its app package (your business logic).
COPY main.py /app/main.py
COPY app     /app/app

# The default destination root of a `local` sink. Created here, owned by the runtime UID, so that
# whatever is mounted over it can be written to: an empty Docker named volume inherits the ownership
# of the image path it covers (compose.yaml), and a Kubernetes emptyDir is mounted writable
# (k8s/deployment.yaml). Point a sink somewhere else and mount that path instead.
RUN mkdir -p /data/out && chown -R 65532:65532 /data

# Drop to a non-root UID; /tmp is provided writable by the Deployment (emptyDir).
USER 65532:65532

ENTRYPOINT ["python3", "/app/main.py"]
# No default args: with --platform auto the library detects KUBERNETES from the SA token
# (config source -> CONFIGMAP at /etc/edgecommons, transport -> MQTT, identity -> Downward API).
