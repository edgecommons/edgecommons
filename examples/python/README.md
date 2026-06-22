# Python Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in Python on top of the
[`ggcommons`](../../libs/python) Python library. It is the Python counterpart of `examples/java`,
`examples/rust`, and `examples/ts`, demonstrating the library's standard CLI contract,
configuration, logging, messaging (publish + request/reply), metrics, and heartbeat — so a component
author writes only business logic (in [`app/greengrass_app.py`](app/greengrass_app.py)).

The component is `aws.proserve.greengrass.PythonComponentSkeleton` and is bootstrapped via
`GGCommonsBuilder.create(...)` in [`main.py`](main.py).

## Run locally (STANDALONE mode)

Bring up a local MQTT broker, then run the component against it:

```bash
docker compose -f ../../test-infra/compose.yaml up -d        # EMQX broker on :1883
pip install -r requirements.txt
python3 main.py -m STANDALONE test-configs/standalone-local.json -c FILE test-configs/config_2.json -t my-thing-name
```

Subscribe to `heartbeat/+/+` (e.g. with MQTTX) to see the component's heartbeats, and to its
request/response topics to drive it.

## Run under Greengrass

In GREENGRASS mode (the default) the component reads its config from the deployment:

```bash
python3 main.py -c GG_CONFIG -t my-thing-name
```

Package and deploy with the **GDK** using the bundled `gdk-config.json` and `recipe.yaml`:

```bash
gdk component build
gdk component publish
```

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG` (default), `SHADOW`, `CONFIG_COMPONENT`.
- `-m/--mode <MODE> [path]` — `GREENGRASS` (default) or `STANDALONE <messaging_config.json>`.
- `-t/--thing <name>` — IoT Thing name.

## Layout

| Path | What it is |
|------|-----------|
| `main.py` | Entry point — builds `GGCommons` and starts the app. |
| `app/greengrass_app.py` | The business logic (config, messaging, metrics, heartbeat). |
| `test-configs/` | Sample config files (`config_*.json`) + the STANDALONE messaging config (`standalone-local.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass component recipe + GDK build/publish config. |
| `tests/` | Local tests for the example. |

To scaffold a fresh component instead of copying this one, use the CLI:
`ggcommons create-component -l PYTHON -n com.example.MyComponent`.
