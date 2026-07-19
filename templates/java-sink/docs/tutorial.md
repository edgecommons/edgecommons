# Tutorial — Deliver to the Local Reference Destination

> This documents the generated scaffold; rewrite it as you build the component out.

This tutorial builds and runs the scaffold's demo sink, publishes a message for it to consume, and
watches it land — idempotently, verified — on the local filesystem.

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

The bundled `test-configs/config.json` declares one sink, `archive`: subscribes
`ecv1/+/+/+/data/#`, delivers to `./out` on the local filesystem.

```bash
java -jar target/<<JARNAME>>-1.0.0.jar \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json -t tutorial-thing
```

## Step 3 — Watch the event ladder

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/+/evt/#' -v
```

## Step 4 — Publish something for it to deliver

```bash
mosquitto_pub -h localhost -t 'ecv1/site1/some-adapter/dev1/data/temperature-1' -m \
  '{"header":{"name":"SouthboundSignalUpdate","version":"1.0","uuid":"11111111-1111-1111-1111-111111111111"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.5,"quality":"GOOD"}]}}'
```

Watch `delivery-started` then `delivery-completed` on the event stream, and check `./out` — a file
named after the sink id, topic leaf, and the message's envelope uuid now holds the delivered payload.

## Step 5 — Prove idempotent redelivery

Publish the **exact same message** again (same `uuid`). It lands at the **same key** — the file is
overwritten, not duplicated — because a sink that cannot retry without duplicating cannot retry at
all.

## Step 6 — Watch the delivery metric

```bash
mosquitto_pub -h localhost -t 'ecv1/tutorial-thing/<<BINNAME>>/cmd/ping' -m \
  '{"header":{"name":"ping","version":"1.0","reply_to":"app/reply/1","correlation_id":"1"}}'
```

Or, with `metricEmission.target: messaging`, subscribe `ecv1/+/+/metric/#` and watch
`sinkDeliveries` (`received`/`delivered`/`retried`/`exhausted`/`dropped`) flush.

## Step 7 — Clean up

Stop the component with Ctrl-C, remove `./out`, and remove the broker:

```bash
docker rm -f emqx
```

## What you did

You ran the scaffold's one demo sink, watched its event ladder report a successful, verified
delivery, and proved that redelivering the same message overwrites rather than duplicates.

## Next steps

- Add a real destination: [How-to — Add a destination](how-to-guides.md#add-a-destination).
- Understand why the ordering (deliver → verify → confirm → report) is the whole archetype: [Explanation](explanation.md).
- Look up every event and payload shape: [Reference — Messaging Interface](reference/messaging-interface.md).
