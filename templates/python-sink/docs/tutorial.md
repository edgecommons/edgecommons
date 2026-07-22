# Tutorial — From zero to a delivered object

*This documents the generated scaffold; rewrite it as you build the component out.*

By the end you'll have `<<COMPONENTNAME>>` running one sink that delivers messages to the local
filesystem, watched it verify and confirm a delivery, and seen a redelivery overwrite rather than
duplicate.

## 1. Prerequisites

- Python 3.9+, and a local MQTT broker on `localhost:1883`
  (`docker run -d -p 1883:1883 emqx/emqx:latest`, or `docker compose up --build`, which starts one
  for you alongside the sink).
- From this directory: `pip install -r requirements.txt`.

## 2. Run it

```bash
python3 main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t my-thing
```

`test-configs/config.json` declares one sink, `archive`: it subscribes to `ecv1/+/+/+/data/#` and
delivers each message to `./out` on the local filesystem (`destination: {"type": "local", "path":
"./out"}`).

## 3. Give it something to deliver

```bash
mosquitto_pub -h localhost -p 1883 -t 'ecv1/gw-01/sim/data/temperature-1' \
  -m '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"}}}'
```

Then check where it landed:

```bash
ls -R ./out            # archive/temperature-1/<uuid>.json
cat ./out/archive/temperature-1/*.json
```

The key (`archive/temperature-1/<uuid>.json`) is deterministic — the same message always resolves to
the same key. Publish the exact same message again (same header uuid, if you script it) and confirm
it overwrites rather than creating a second file.

## 4. Watch the event ladder

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/+/evt/#' -v
```

For the delivery above you'll see, in order: `evt/info/delivery-started`, then
`evt/info/delivery-completed` — never `delivery-completed` without a `delivery-started` first, and
never a `delivery-exhausted` on a successful local delivery (there's nothing to retry against).

## 5. Watch the reported surface

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/state' -t 'ecv1/+/+/metric/#' -v
```

- **`state`** — the automatic keepalive, this time **with** an `instances[]` array: one entry per
  configured sink (`archive`), because a sink's destinations *are* its instances — configured from
  startup, before a single message arrives (see [explanation.md](explanation.md)).
- **`metric/sinkDeliveries`** — `received`/`delivered`/`retried`/`exhausted`/`dropped`, once a minute.

## 6. Run the tests

```bash
python -m pytest
```

No broker needed. `tests/test_dest.py` exercises the destination, the retry policy, and the health
states as pure logic; `tests/test_connectivity.py` exercises the one seam the app wires into the
library.

Next: the [how-to guides](how-to-guides.md) to add a real destination and deploy; the
[reference](reference/) for every option and topic; the [explanation](explanation.md) for why the
sink is shaped this way.
