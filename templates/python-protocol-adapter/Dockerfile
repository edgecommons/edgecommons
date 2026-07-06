# Container image for the <<COMPONENTNAME>> edgecommons protocol adapter on Kubernetes.
#
# The edgecommons Python library is resolved from requirements.txt (`edgecommons`). Build from
# this directory, push or `kind load`, then set `image:` in k8s/deployment.yaml:
#   docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .
FROM python:3.12-slim

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

WORKDIR /app

COPY requirements.txt /app/requirements.txt
RUN pip install --no-cache-dir -r /app/requirements.txt

COPY main.py /app/main.py
COPY app     /app/app

USER 65532:65532

ENTRYPOINT ["python3", "/app/main.py"]
# No default args: --platform auto detects KUBERNETES (CONFIGMAP source at /etc/edgecommons, MQTT
# transport, Downward-API identity).
