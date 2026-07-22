This documents the generated scaffold; rewrite it as you build the component out.

# Tutorial — From zero to live values

By the end you'll have `<<COMPONENTNAME>>` polling its built-in simulator and publishing readings
onto a local MQTT broker, and you'll have read a signal, written one, and watched the health
metric move. No hardware required — the simulator ships in `src/device.ts`.

## 1. Prerequisites

- Node.js 20+, and a local MQTT broker on `localhost:1883` (`docker run -d -p 1883:1883 emqx/emqx`).
- The sibling `edgecommons` TypeScript library built (`npm run build` in `libs/ts`) — this scaffold
  depends on it via a `file:` path (`--dep-source local`, the default).

## 2. Install and build

```bash
npm install
npm run build
```

## 3. Run it

```bash
node dist/main.js \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

`test-configs/config.json` configures one instance (`device-1`) against the `sim` backend, polling
every 5 seconds. You should see it connect immediately (the simulator never fails to connect once
an endpoint string is set) and start publishing.

## 4. Watch values flow

Subscribe to the UNS `data` class — one wildcard covers the whole fleet:

```bash
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/data/#' -v
```

You'll see a `SouthboundSignalUpdate` for `temperature-1` (a sine wave, `GOOD` quality) every poll,
and one for `pressure-1` (always `BAD`, `qualityRaw: "SENSOR_FAULT"`) — the simulator ships a
failing signal on purpose, so you see from the first run that a read failure is *reported*, not
silently dropped. Also try:

```bash
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/state' -v      # the keepalive + per-instance connectivity
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/metric/#' -v   # southbound_health + the two operational families
```

## 5. Read a signal on demand

The `sb/*` command surface rides the library's command inbox
(`ecv1/{device}/{component}/cmd/{verb}`). Set `header.name` to the verb and `header.reply_to` to a
topic you subscribe:

```
publish ecv1/my-thing/<<COMPONENTNAME>>/cmd/sb/read
  {"header":{"name":"sb/read","reply_to":"app/r","correlation_id":"1"},
   "body":{"signals":[{"name":"temperature-1"}]}}
subscribe app/r → {"ok":true,"result":{"id":"device-1","reads":[{"signal":{"id":"temperature-1"},"value":21.3,"quality":"GOOD",...}]}}
```

## 6. Write a signal

The scaffold's `test-configs/config.json` ships an **empty** `writes.allow` list — writes are
refused by default, on purpose (an adapter that writes whatever it's asked is a control-system
risk, not a feature). Add the signal you want to allow:

```jsonc
"writes": { "allow": ["temperature-1"] }
```

Rebuild the config, restart, then:

```
publish ecv1/my-thing/<<COMPONENTNAME>>/cmd/sb/write
  {"header":{"name":"sb/write","reply_to":"app/r","correlation_id":"2"},
   "body":{"writes":[{"signalId":"temperature-1","value":42.5}]}}
subscribe app/r → {"ok":true,"result":{"id":"device-1","written":1,"results":[...]}}
```

The simulator's `writeSignal` just logs the write — a real backend would send it to the device.

## 7. Browse the device's address space

```
publish ecv1/my-thing/<<COMPONENTNAME>>/cmd/sb/browse
  {"header":{"name":"sb/browse","reply_to":"app/r","correlation_id":"3"},"body":{}}
subscribe app/r → {"ok":true,"result":{"id":"device-1","entries":[{"id":"temperature-1",...},{"id":"pressure-1",...}]}}
```

## 8. Run the tests

```bash
npm test
```

Every suite runs with no external dependencies (the simulator IS the test fixture). One suite,
`test/live-sim.test.ts`, is **skipped** unless `EC_LIVE_SIM` is set — see
[how-to guides](how-to-guides.md#point-the-live-sim-suite-at-a-real-device) once you've replaced
the simulator with a real protocol.

Next: the [how-to guides](how-to-guides.md) for adding devices, replacing the simulator, and
deploying; the [reference](reference/) for every option; the [explanation](explanation.md) for the
model behind the seam.
