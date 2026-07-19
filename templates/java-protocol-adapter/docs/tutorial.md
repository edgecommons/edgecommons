# Tutorial — Run the Adapter Against Its Simulator

> This documents the generated scaffold; rewrite it as you build the component out.

This tutorial takes the scaffold from nothing to a running adapter that publishes simulated device
readings onto a message bus, then has you read and write a signal through its command surface. No
real hardware is needed — the scaffold ships with an in-process simulated device
(`Device.SimBackend`) for exactly this purpose.

## Prerequisites

- **Java 25** and **Maven**.
- An **MQTT broker** reachable on `localhost:1883` — EMQX in Docker is the easiest:
  `docker run -d --name emqx -p 1883:1883 emqx/emqx:latest`.
- A small MQTT client to watch traffic — `mosquitto_sub`/`mosquitto_pub`, MQTTX, or a short Python
  script with `paho-mqtt`.

Run everything from the component root.

## Step 1 — Build

```bash
mvn -q clean package
```

You get `target/<<JARNAME>>-1.0.0.jar`.

## Step 2 — Run

```bash
java -jar target/<<JARNAME>>-1.0.0.jar \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/<<COMPONENTNAME>>.json -t tutorial-thing
```

The bundled config (`test-configs/<<COMPONENTNAME>>.json`) declares one device, `device-1`, using the
`sim` adapter. Watch the log for the connect and the first poll cycle.

## Step 3 — Watch signal updates (the data plane)

Subscribe to the adapter's output:

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/+/data/#' -v
```

Within a few seconds you see JSON messages carrying a `SouthboundSignalUpdate` body — the simulated
`temperature-1` signal riding a sine wave with `quality: GOOD`, and `pressure-1` published with
`quality: BAD` (`SENSOR_FAULT`) on purpose, to show that a failed read is reported, not dropped.

## Step 4 — Check status

```bash
mosquitto_pub -h localhost -t 'ecv1/tutorial-thing/<<BINNAME>>/cmd/sb/status' -m \
  '{"header":{"name":"sb/status","reply_to":"app/reply/1","correlation_id":"1"},"body":{"instance":"device-1"}}'
mosquitto_sub -h localhost -t 'app/reply/1' -C 1 -v
```

The reply's `result` carries `connected`, `state`, `paused`, `endpoint`, and the device's counters.

## Step 5 — Read a signal on demand

```bash
mosquitto_pub -h localhost -t 'ecv1/tutorial-thing/<<BINNAME>>/cmd/sb/read' -m \
  '{"header":{"name":"sb/read","reply_to":"app/reply/2","correlation_id":"2"},"body":{"instance":"device-1","signals":[{"signalId":"temperature-1"}]}}'
mosquitto_sub -h localhost -t 'app/reply/2' -C 1 -v
```

## Step 6 — Attempt a write

```bash
mosquitto_pub -h localhost -t 'ecv1/tutorial-thing/<<BINNAME>>/cmd/sb/write' -m \
  '{"header":{"name":"sb/write","reply_to":"app/reply/3","correlation_id":"3"},"body":{"instance":"device-1","writes":[{"signalId":"temperature-1","value":21.0}]}}'
mosquitto_sub -h localhost -t 'app/reply/3' -C 1 -v
```

The bundled config's `writes.allow` is empty, so this comes back `WRITE_NOT_ALLOWED` — the correct,
secure-by-default behavior. Add `"temperature-1"` to `writes.allow` in
`test-configs/<<COMPONENTNAME>>.json` and re-run to see it succeed instead.

## Step 7 — Clean up

Stop the adapter with Ctrl-C, and remove the broker:

```bash
docker rm -f emqx
```

## What you did

You ran the scaffold end to end: watched it publish simulated readings as `SouthboundSignalUpdate`
messages, queried its status, and exercised the `sb/read`/`sb/write` command surface — including the
allow-list rejecting an unlisted write. The whole interaction happened over the bus.

## Next steps

- Replace the simulator with a real protocol: [How-to — Implement a real device backend](how-to-guides.md#implement-a-real-device-backend).
- Understand why the seam is shaped the way it is: [Explanation](explanation.md).
- Look up every verb and payload shape: [Reference — Messaging Interface](reference/messaging-interface.md).
