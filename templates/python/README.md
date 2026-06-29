# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Python on top of the
`ggcommons` (`greengrass-commons`) Python library, generated from the GGCommons Python component
template by the `ggcommons` CLI. It gives you the library's standard CLI contract, configuration,
logging, messaging, metrics, and heartbeat — so you write only business logic in
[`app/<<COMPONENTNAME>>.py`](app/<<COMPONENTNAME>>.py).

## Run locally (HOST platform, MQTT transport)

```bash
pip install -r requirements.txt
# Provide an MQTT messaging-config JSON (messaging.local required, messaging.iotCore optional):
python3 main.py --platform HOST --transport MQTT ./standalone-messaging.json -c FILE test-configs/config_2.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`heartbeat/+/+` to see the component's heartbeats.

## Run under Greengrass

On the GREENGRASS platform the component reads its config from the deployment:

```bash
python3 main.py --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

## Deploy to Greengrass

Packaged with the **GDK (Greengrass Development Kit)** using `gdk-config.json` and `recipe.yaml`:

```bash
gdk component build
gdk component publish
```

## Deploy to Kubernetes

The Kubernetes artifacts (`Dockerfile`, `k8s/`) exist only when this component was scaffolded
with **KUBERNETES** as a target platform. Build the image from `./Dockerfile`, make it available
to the cluster, point `image:` at it, then apply the manifests:

```bash
# 1. Build the image (requirements.txt resolves the published ggcommons library).
docker build -t ghcr.io/<owner>/<<COMPONENTNAME>>:latest .

# 2. Make it available to the cluster — push to a registry...
docker push ghcr.io/<owner>/<<COMPONENTNAME>>:latest
#    ...or, for a local kind cluster, load it directly:
# kind load docker-image ghcr.io/<owner>/<<COMPONENTNAME>>:latest

# 3. Set `image:` in k8s/deployment.yaml (replace REPLACE_ME) to that image, then:
kubectl apply -f k8s/
```

With `--platform auto` the library detects KUBERNETES from the ServiceAccount token, reads its
config from the mounted ConfigMap (`CONFIGMAP` source, hot-reloaded on `kubectl apply`), uses the
MQTT transport from that same ConfigMap, and resolves identity from the Downward API — so the
Deployment needs no command-line args.

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`).
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS).
- `-t/--thing <name>` — IoT Thing name.

## Layout

| Path | What it is |
|------|-----------|
| `main.py` | Entry point — builds `GGCommons` and starts the app. |
| `app/<<COMPONENTNAME>>.py` | Your business logic. |
| `test-configs/` | Sample component-config files (`config_*.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
