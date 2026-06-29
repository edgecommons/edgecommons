# <<COMPONENTNAME>>

A **Python protocol-adapter** component (`<<COMPONENTFULLNAME>>`) built on the `ggcommons`
(`greengrass-commons`) library and the **southbound contract** (`docs/SOUTHBOUND.md`). It bridges a
device protocol onto a message bus: it polls or subscribes a source and republishes value changes as
`SouthboundTagUpdate` messages, optionally serves on-demand reads/writes, and emits the
`southbound_health` metric. Runs as a Greengrass v2 component, a standalone process, or a Kubernetes
pod.

This is a **scaffold** — implement your protocol in `app/<<COMPONENTNAME>>.py` (search for `TODO`).
See the OPC UA (subscribe-based, Java) and Modbus (poll-based, Python) reference adapters for complete
implementations.

## Layout

- `main.py` — builds ggcommons, spawns one worker per `component.instances[]` entry.
- `app/<<COMPONENTNAME>>.py` — your adapter: connect, poll/subscribe, publish, command surface.
- `requirements.txt` — `greengrass-commons` + your protocol client library.
- `recipe.yaml` / `gdk-config.json` — Greengrass packaging (IPC pubsub access; venv install).
- `Dockerfile` / `k8s/` — Kubernetes image + manifests (KUBERNETES platform only).
- `test-configs/<<COMPONENTNAME>>.json` — a sample config following the §4 convention.

## Run locally (HOST)

```bash
pip install -e . -r requirements.txt
python main.py --platform HOST --transport MQTT ./standalone-messaging.json \
       -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing
```

## CLI contract

`--platform GREENGRASS|HOST|KUBERNETES|auto` · `--transport IPC|MQTT [path]` ·
`-c/--config FILE <path>|ENV|GG_CONFIG|CONFIGMAP|…` · `-t/--thing <name>`.
