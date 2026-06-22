# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Python on top of the
`ggcommons` (`greengrass-commons`) Python library, generated from the GGCommons Python component
template by the `ggcommons` CLI. It gives you the library's standard CLI contract, configuration,
logging, messaging, metrics, and heartbeat — so you write only business logic in
[`app/<<COMPONENTNAME>>.py`](app/<<COMPONENTNAME>>.py).

## Run locally (STANDALONE mode)

```bash
pip install -r requirements.txt
# Provide a STANDALONE messaging-config JSON (messaging.local required, messaging.iotCore optional):
python3 main.py -m STANDALONE ./standalone-messaging.json -c FILE test-configs/config_2.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`heartbeat/+/+` to see the component's heartbeats.

## Run under Greengrass

In GREENGRASS mode (the default) the component reads its config from the deployment:

```bash
python3 main.py -c GG_CONFIG -t my-thing-name
```

## Build & publish

Packaged with the **GDK (Greengrass Development Kit)** using `gdk-config.json` and `recipe.yaml`:

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
| `app/<<COMPONENTNAME>>.py` | Your business logic. |
| `test-configs/` | Sample component-config files (`config_*.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
