# Tutorial — From zero to live values

*This documents the generated scaffold; rewrite it as you build the component out.*

By the end you'll have `<<COMPONENTNAME>>` polling its built-in **simulator**, publishing value
changes onto MQTT, and answering the `sb/*` command surface. No hardware, no broker config beyond a
local MQTT broker.

## 1. Prerequisites

- Python 3.9+, and a local MQTT broker on `localhost:1883`
  (`docker run -d -p 1883:1883 emqx/emqx:latest`, or `docker compose up --build`, which starts one
  for you).
- From this directory: `pip install -e . -r requirements.txt` (or just `pip install -r
  requirements.txt` — `pyproject.toml`'s `pythonpath = ["."]` also resolves the package under
  `pytest` without an install).

## 2. Run it

```bash
python main.py --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE test-configs/<<COMPONENTNAME>>.json -t my-thing
```

`test-configs/<<COMPONENTNAME>>.json` declares one device, `device-1`, with `adapter: "sim"` — the
built-in simulator, which exposes two signals: `temperature-1` (a sine-wave reading that always
succeeds) and `pressure-1` (always reported `BAD`/`SENSOR_FAULT`, on purpose — a read failure is
information, and this scaffold never lets one dead signal hide behind silence). Writes are disabled
by default (`writes.allow: []`).

## 3. Watch values flow

Subscribe to the UNS wildcards (any MQTT client):

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/+/data/#' -t 'ecv1/+/+/state' -t 'ecv1/+/+/metric/#' -v
```

You'll see `SouthboundSignalUpdate` messages on `ecv1/my-thing/<<BINNAME>>/device-1/data/{signal}`
for both signals — `temperature-1` with `quality: GOOD`, `pressure-1` with `quality: BAD` and
`qualityRaw: SENSOR_FAULT` — plus the `state` keepalive (with a `device-1` entry in `instances[]`)
and, once `metricEmission.target` is `messaging`, `southbound_health` and the
`<<COMPONENTNAME>>Connection`/`<<COMPONENTNAME>>Command` operational metric families.

## 4. Check status and the signal inventory

```
publish ecv1/my-thing/<<BINNAME>>/cmd/sb/status
  {"header":{"name":"sb/status","reply_to":"app/r","correlation_id":"1"},"body":{}}
subscribe app/r  →  {"ok":true,"result":{"id":"device-1","connected":true,"state":"ONLINE",...}}

publish ecv1/my-thing/<<BINNAME>>/cmd/sb/signals
  {"header":{"name":"sb/signals","reply_to":"app/r","correlation_id":"2"},"body":{}}
subscribe app/r  →  {"ok":true,"result":{"id":"device-1","signals":[
  {"id":"temperature-1","name":"Ambient temperature","writable":false},
  {"id":"pressure-1","name":"Line pressure","writable":false}]}}
```

`instance` is omitted from both bodies because exactly one device is configured — see
[explanation.md](explanation.md#instance-routing).

## 5. Read a signal on demand

```
publish ecv1/my-thing/<<BINNAME>>/cmd/sb/read
  {"header":{"name":"sb/read","reply_to":"app/r","correlation_id":"3"},
   "body":{"signals":[{"name":"temperature-1"}]}}
subscribe app/r  →  {"ok":true,"result":{"id":"device-1","reads":[
  {"signal":{"id":"temperature-1"},"value":21.7,"quality":"GOOD","qualityRaw":"OK"}]}}
```

## 6. Try a write (and see it refused)

The default config's `writes.allow` is empty, so every write is refused **before it ever reaches the
device** — the allow-list is checked first, always:

```
publish ecv1/my-thing/<<BINNAME>>/cmd/sb/write
  {"header":{"name":"sb/write","reply_to":"app/r","correlation_id":"4"},
   "body":{"writes":[{"signalId":"temperature-1","value":25.0}]}}
subscribe app/r  →  {"ok":false,"error":{"code":"WRITE_NOT_ALLOWED","message":"..."}}
```

Add `"temperature-1"` to `writes.allow` in the config, restart, and the same request now succeeds
(the simulator accepts any write): `{"ok":true,"result":{"id":"device-1","written":1,"results":[...]}}`.

## 7. Browse, reconnect, pause/resume

```
publish ecv1/my-thing/<<BINNAME>>/cmd/sb/browse
  {"header":{"name":"sb/browse","reply_to":"app/r","correlation_id":"5"},"body":{}}
subscribe app/r  →  {"ok":true,"result":{"id":"device-1","entries":[
  {"id":"temperature-1","name":"Ambient temperature","type":"REAL"},
  {"id":"pressure-1","name":"Line pressure","type":"REAL"}]}}
```

`reconnect` drops and re-establishes the (simulated) session; `repoll` forces an immediate poll
cycle; `sb/pause`/`sb/resume` stop and resume telemetry production without dropping the connection —
try `sb/pause` and confirm `data` publishes stop while `sb/status` still answers.

## 8. Run the tests

```bash
python -m pytest
```

No broker needed — `tests/test_commands.py`, `tests/test_device.py`, and `tests/test_metrics.py`
exercise every verb, error code, the allow-list, and the metric measure names against a mock control
seam. `EC_LIVE_SIM` is unset, so `tests/test_live_sim.py` **skips** (see
[how-to-guides.md](how-to-guides.md#run-the-live-sim-integration-test)).

Next: the [how-to guides](how-to-guides.md) to implement a real protocol and deploy; the
[reference](reference/) for every option, topic, and type; the [explanation](explanation.md) for the
model.
