# <<COMPONENTNAME>>

An AWS IoT Greengrass v2 component (`<<COMPONENTFULLNAME>>`) written in Java on top of the
`ggcommons` Java library, generated from the GGCommons Java component template by the `ggcommons`
CLI. It gives you the library's standard CLI contract, configuration, logging, messaging, metrics,
and heartbeat — so you write only business logic in your component class
(`src/main/java/.../<<COMPONENTNAME>>.java`).

## Run locally (HOST platform, MQTT transport)

```bash
mvn clean package
# Provide an MQTT messaging-config JSON (messaging.local required, messaging.iotCore optional):
java -jar target/<<JARNAME>>-1.0.0.jar --platform HOST --transport MQTT ./standalone-messaging.json -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing-name
```

Needs a local MQTT broker (e.g. `docker run -d -p 1883:1883 emqx/emqx:latest`). Subscribe to
`heartbeat/+/+` to see the component's heartbeats. If you enable the telemetry-streaming subsystem,
add `--enable-native-access=ALL-UNNAMED` (the FFM/Panama binding to `ggstreamlog`).

## Run under Greengrass

On the GREENGRASS platform the component reads its config from the deployment:

```bash
java -jar target/<<JARNAME>>-1.0.0.jar --platform GREENGRASS -c GG_CONFIG -t my-thing-name
```

## Build & publish

Built with **Maven** (a shaded, self-contained JAR) and packaged with the **GDK (Greengrass
Development Kit)** using `gdk-config.json` and `recipe.yaml`:

```bash
mvn clean package
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
| `src/main/java/.../<<COMPONENTNAME>>.java` | Your business logic. |
| `pom.xml` | Maven build (shaded JAR for Greengrass deployment). |
| `test-configs/` | Sample component-config files (`<<COMPONENTNAME>>.json`). |
| `recipe.yaml`, `gdk-config.json` | Greengrass recipe + GDK build/publish config. |
