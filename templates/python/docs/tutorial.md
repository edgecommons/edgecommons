# Tutorial — From zero to a live component

*This documents the generated scaffold; rewrite it as you build the component out.*

By the end you'll have `<<COMPONENTNAME>>` running against a local MQTT broker, watched its
heartbeat, metric, data signal, and event land on the bus, and invoked its one custom command verb.
No device and no cloud account required.

## 1. Prerequisites

- Python 3.9+, and a local MQTT broker on `localhost:1883`
  (`docker run -d -p 1883:1883 emqx/emqx:latest`).
- From this directory: `pip install -r requirements.txt`.

## 2. Run it

```bash
python3 main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE test-configs/config_1.json -t my-thing
```

You should see it connect to the broker, log "Starting <<COMPONENTNAME>>", and start publishing on
a 2-second loop (`component.global.publish_interval` in the config). `test-configs/config_1.json`
declares no `component.instances[]` — the scaffold runs fine with zero instances, since it owns no
southbound connections of its own.

## 3. Watch it on the bus

Subscribe to the six UNS wildcards (any MQTT client, e.g. `mosquitto_sub` or MQTTX):

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/state' -t 'ecv1/+/+/metric/#' \
              -t 'ecv1/+/+/data/#' -t 'ecv1/+/+/evt/#' -v
```

You'll see:

- **`state`** — the library's automatic keepalive, every ~5 s (RUNNING, with no `instances[]`
  section since the scaffold reports none — see [explanation.md](explanation.md)).
- **`metric/loopTicks`** — a monotonic `tickCount` counter plus an `uptimeSecs` measure, once per
  loop (only publishes over MQTT when `metricEmission.target` is `messaging`; `config_1.json` uses
  the default `log` target, so also try `config_2.json` or set `target: messaging` to see it here).
- **`data/demo-signal`** — a sine-wave reading, standing in for a real sensor value.
- **`evt/info/sample-event`** — a discrete occurrence, carrying the current greeting in its context.

## 4. Invoke the custom command

The scaffold registers one command verb, `set-greeting`, alongside the library's automatic
`ping`/`reload-config`/`get-configuration`. Publish a request and subscribe to the reply topic:

```
publish ecv1/my-thing/<<BINNAME>>/main/cmd/set-greeting
  {"header":{"name":"set-greeting","version":"1.0","reply_to":"app/r","correlation_id":"1"},
   "body":{"greeting":"Hi there"}}
subscribe app/r  →  {"ok":true,"result":{"previousGreeting":"Hello from <<COMPONENTNAME>>","greeting":"Hi there"}}
```

The next `app/status` publish reflects the new greeting — a command's effect is visibly observable
on the very next tick, without a dedicated "get" verb.

## 5. Run the tests

```bash
python -m pytest
```

No broker needed — `tests/test_app.py` exercises the custom verb and the instance-connectivity
provider against a recording stand-in for the framework.

Next: the [how-to guides](how-to-guides.md) to replace the demo state with real logic and deploy;
the [reference](reference/) for every option and topic; the [explanation](explanation.md) for why
the scaffold is shaped this way.
