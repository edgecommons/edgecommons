# <<COMPONENTNAME>>

A **Python protocol-adapter** component (`<<COMPONENTFULLNAME>>`) built on the `edgecommons`
library and the **southbound contract** (`docs/SOUTHBOUND.md`). It bridges a device protocol onto the
unified namespace (UNS): it polls a source and republishes value changes as `SouthboundSignalUpdate`
messages on per-instance UNS `data` topics (`ecv1/{device}/{component}/{instance}/data/{signalPath}`,
via the `data()` facade), serves the standardized `sb/*` command surface, and emits
`southbound_health`. Runs as a Greengrass v2 component, a standalone process, or a Kubernetes pod.

Out of the box it runs against an **in-process simulator** (`adapter: sim`) — no PLC, no hardware —
so a fresh scaffold connects, publishes, and answers commands on the first run. Replace the simulator
with your protocol behind the same seam.

## Layout

The package is `<<SNAKENAME>>/` (like `modbus_adapter/`), one file per concern:

- `main.py` — builds edgecommons and hands off to `<<SNAKENAME>>.adapter.App`.
- `<<SNAKENAME>>/device.py` — the **protocol seam** (`DeviceBackend` / `DeviceSession`) plus the
  in-process simulator. Implement this once per protocol; nothing above it learns your protocol.
- `<<SNAKENAME>>/adapter.py` — the connect/poll/reconnect worker (`App`, `Device`), the per-device
  `Health`, the write allow-list, and the instance-connectivity the `state` keepalive + `status` verb
  report.
- `<<SNAKENAME>>/metrics.py` — `southbound_health` (the §5 canonical set) + the operational-family
  pattern (`<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command`), with a signposted place to add
  your protocol's `Inventory`/`Poll`/`Publish` families.
- `<<SNAKENAME>>/command_service.py` — the `sb/*` command surface + the three edge-console panels.
- `tests/` — `pytest` tests for every verb, each error code, the allow-list, the panels, and the
  metric set. `python -m pytest` — no broker and no device needed.
- `requirements.txt` / `pyproject.toml` — `edgecommons` + your protocol client library.
- `recipe.yaml` / `gdk-config.json` — Greengrass packaging (IPC pubsub access; venv install).
- `Dockerfile` / `k8s/` — Kubernetes image + manifests (KUBERNETES platform only).
- `test-configs/<<COMPONENTNAME>>.json` — a sample config following the §4 convention.

## The command surface (`sb/*`)

Every verb is served on the library command inbox
(`ecv1/{device}/{component}[/{instance}]/cmd/sb/{verb}`) and routes to the addressed device by the
request body's `instance` (optional when exactly one device is configured):

| Verb | What it does |
|---|---|
| `sb/status` | Link state / paused / endpoint + a counter snapshot. |
| `sb/read` | On-demand read of named signals (`{signals:[{signalId|id|name}]}`). |
| `sb/write` | Batch write (`{writes:[{signalId, value}]}`), **allow-listed before any device I/O**. |
| `sb/signals` | The configured signal inventory (no device round-trip). |
| `sb/browse` | Paged address-space discovery (the sim returns one page; `BROWSE_UNSUPPORTED` when a protocol has none). |
| `sb/pause` / `sb/resume` | Pause/resume telemetry production (idempotent). |
| `reconnect` | Drop + re-establish the link, one attempt. |
| `repoll` | Force an immediate poll (refused while paused). |

Writes are **allow-listed by stable `signal.id`** (`writes.allow` per device) and the list is checked
**before** anything reaches the device — an empty list (the default) means the adapter is read-only.

## Run locally (HOST)

```bash
pip install -e . -r requirements.txt
python main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
       -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing
```

Watch it on the bus (e.g. with MQTTX): subscribe to `ecv1/+/+/state` for the component's keepalives
(with per-device connectivity in `instances[]`) and `ecv1/+/+/+/data/#` for the published signal
updates (one instance per device).

## CLI contract

`--platform GREENGRASS|HOST|KUBERNETES|auto` · `--transport IPC|MQTT [path]` ·
`-c/--config FILE <path>|ENV|GG_CONFIG|CONFIGMAP|…` · `-t/--thing <name>`.
