# <<COMPONENTNAME>>

A **Python protocol-adapter** component (`<<COMPONENTFULLNAME>>`) built on the `edgecommons`
(`edgecommons`) library and the **southbound contract** (`docs/SOUTHBOUND.md`). It bridges a
device protocol onto the unified namespace (UNS): it polls or subscribes a source and republishes
value changes as `SouthboundSignalUpdate` messages on per-instance UNS `data` topics
(`ecv1/{device}/{component}/{instance}/data/{signalPath}`, minted via `gg.instance(id).uns()`),
optionally serves on-demand reads/writes, and emits the `southbound_health` metric. Runs as a
Greengrass v2 component, a standalone process, or a Kubernetes pod.

The component's place in the namespace comes from the top-level `hierarchy` + `identity` config
blocks (the last hierarchy level is always the resolved thing name); every published envelope
carries the matching `identity` element, stamped automatically.

This is a **scaffold** — implement your protocol in `app/<<COMPONENTNAME>>.py` (search for `TODO`).
See the OPC UA (subscribe-based, Java) and Modbus (poll-based, Python) reference adapters for complete
implementations.

## Layout

- `main.py` — builds edgecommons, registers the instance-connectivity provider, spawns one worker per
  `component.instances[]` entry.
- `app/<<COMPONENTNAME>>.py` — your adapter: connect, poll/subscribe, publish, command surface, and
  each device's `LinkStatus` (what the `state` keepalive and the `status` verb report).
- `tests/` — `pytest` tests for what the adapter reports about its devices. `python -m pytest` — no
  broker and no device needed.
- `requirements.txt` — `edgecommons` + your protocol client library.
- `recipe.yaml` / `gdk-config.json` — Greengrass packaging (IPC pubsub access; venv install).
- `Dockerfile` / `k8s/` — Kubernetes image + manifests (KUBERNETES platform only).
- `test-configs/<<COMPONENTNAME>>.json` — a sample config following the §4 convention.

## Run locally (HOST)

```bash
pip install -e . -r requirements.txt
python main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
       -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing
```

Watch it on the bus (e.g. with MQTTX): subscribe to `ecv1/+/+/+/state` for the component's
heartbeat keepalives and `ecv1/+/+/+/data/#` for the published signal updates.

> The on-demand read/write command surface stays on the per-instance `write.topic` / `read.topic`
> subscriptions for now; the standardized southbound command family (`sb/*` verbs) moves to the UNS
> command inbox `ecv1/{device}/{component}/{instance}/cmd/sb/{verb}` in Phase 5 (M9).

## CLI contract

`--platform GREENGRASS|HOST|KUBERNETES|auto` · `--transport IPC|MQTT [path]` ·
`-c/--config FILE <path>|ENV|GG_CONFIG|CONFIGMAP|…` · `-t/--thing <name>`.
