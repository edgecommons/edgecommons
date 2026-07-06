# Python Component Skeleton

A worked-example AWS IoT Greengrass v2 component written in Python on top of the
[`edgecommons`](../../libs/python) Python library. It is the Python counterpart of `examples/java`,
`examples/rust`, and `examples/ts`, demonstrating the library's standard CLI contract,
configuration, logging, messaging (publish + request/reply), metrics, and heartbeat — so a component
author writes only business logic (in [`app/greengrass_app.py`](app/greengrass_app.py)).

The component is `com.mbreissi.edgecommons.PythonComponentSkeleton` and is bootstrapped via
`EdgeCommonsBuilder.create(...)` in [`main.py`](main.py).

## Run locally (HOST platform, MQTT transport)

Bring up a local MQTT broker, then run the component against it:

```bash
docker compose -f ../../test-infra/compose.yaml up -d        # EMQX broker on :1883
pip install -r requirements.txt
python3 main.py --platform HOST --transport MQTT test-configs/standalone-local.json -c FILE test-configs/config_2.json -t my-thing-name
```

Subscribe to `ecv1/+/+/+/state` (e.g. with MQTTX) to see the component's heartbeats, and to
`ecv1/+/+/+/app/#` to see its hello-world messages. All topics are unified-namespace (UNS)
topics minted via `gg.uns()` from the component's config-resolved identity — the top-level
`hierarchy` + `identity` blocks in `test-configs/config_*.json` (the last hierarchy level is
always the resolved thing name from `-t/--thing`).

## Run under Greengrass

On the GREENGRASS platform the component reads its config from the deployment:

```bash
python3 main.py --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

Package and deploy with the **GDK** using the bundled `gdk-config.json` and `recipe.yaml`:

```bash
gdk component build
gdk component publish
```

## CLI contract

- `-c/--config <SOURCE> [args]` — `FILE`, `ENV`, `GG_CONFIG`, `SHADOW`, `CONFIG_COMPONENT` (default: from the resolved platform profile — GREENGRASS → GG_CONFIG, HOST → FILE, KUBERNETES → CONFIGMAP).
- `--platform <PLATFORM>` — `GREENGRASS`, `HOST`, `KUBERNETES`, or `auto` (default `auto`).
- `--transport <TRANSPORT> [path]` — `IPC` or `MQTT [messaging_config.json]` (default: from the platform; IPC only valid on GREENGRASS).
- `-t/--thing <name>` — IoT Thing name.

## Layout

| Path | What it is |
|------|-----------|
| `main.py` | Entry point — builds `EdgeCommons` and starts the app. |
| `app/greengrass_app.py` | The business logic (config, messaging, metrics, heartbeat). |
| `test-configs/` | Sample config files (`config_*.json`) + the MQTT messaging config (`standalone-local.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass component recipe + GDK build/publish config. |
| `tests/` | Local tests for the example. |

To scaffold a fresh component instead of copying this one, use the CLI:
`edgecommons create-component -l PYTHON -n com.example.MyComponent`.
