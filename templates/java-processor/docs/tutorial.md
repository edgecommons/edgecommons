# Tutorial — Run the Demo Route End to End

> This documents the generated scaffold; rewrite it as you build the component out.

This tutorial builds and runs the scaffold's demo route — a filter stage feeding a stateful
rollup — and watches it transform published data messages into a periodic summary.

## Prerequisites

- **Java 25** and **Maven**.
- An MQTT broker on `localhost:1883` — `docker run -d --name emqx -p 1883:1883 emqx/emqx:latest`.
- A small MQTT client — `mosquitto_sub`/`mosquitto_pub` or MQTTX.

Run everything from the component root.

## Step 1 — Build

```bash
mvn -q clean package
```

You get `target/<<JARNAME>>-1.0.0.jar`.

## Step 2 — Run

The bundled `test-configs/config.json` declares one route, `rollup`: subscribes
`ecv1/+/+/+/data/#`, filters for `signal.id == "temperature-1"`, and counts arrivals per tick.

```bash
java -jar target/<<JARNAME>>-1.0.0.jar \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t tutorial-thing
```

## Step 3 — Watch the route's output

```bash
mosquitto_sub -h localhost -t 'ecv1/gw-01/<<BINNAME>>/rollup/data/summary' -v
```

## Step 4 — Publish matching and non-matching data

```bash
mosquitto_pub -h localhost -t 'ecv1/site1/some-adapter/dev1/data/temperature-1' -m \
  '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.5,"quality":"GOOD"}]}}'
mosquitto_pub -h localhost -t 'ecv1/site1/some-adapter/dev1/data/pressure-1' -m \
  '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"pressure-1"},"samples":[{"value":4.1,"quality":"GOOD"}]}}'
```

Only the `temperature-1` message passes the `fieldEquals` filter. Within `tickMs` (10 s by default)
you see a summary message on `ecv1/gw-01/<<BINNAME>>/rollup/data/summary` carrying `{"count": 1,
"last": {...}}` — the `pressure-1` message never counted.

## Step 5 — Watch the throughput metric

```bash
mosquitto_pub -h localhost -t 'ecv1/tutorial-thing/<<BINNAME>>/cmd/get-configuration' -m \
  '{"header":{"name":"get-configuration","version":"1.0","reply_to":"app/reply/1","correlation_id":"1"}}'
```

Or, if `metricEmission.target` is `messaging`, subscribe `ecv1/+/+/metric/#` to watch
`processorThroughput` (`received`/`published`/`dropped`/`errors`) flush every minute.

## Step 6 — Clean up

Stop the component with Ctrl-C, and remove the broker:

```bash
docker rm -f emqx
```

## What you did

You ran the scaffold's one demo route, watched a filter stage drop a non-matching message, watched a
stateful rollup stage emit on its tick rather than on arrival, and saw the result republished with
this component's own identity restamped onto it.

## Next steps

- Add your own transformation: [How-to — Add a stage](how-to-guides.md#add-a-stage).
- Understand why `getMessaging()` and not `getData()`, and the two guards: [Explanation](explanation.md).
- Look up every config key: [Reference — Configuration](reference/configuration.md).
