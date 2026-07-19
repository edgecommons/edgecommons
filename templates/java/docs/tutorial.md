# Tutorial — Run the Scaffold and Watch Its Demo Surface

> This documents the generated scaffold; rewrite it as you build the component out.

This tutorial builds and runs the scaffold, then has you watch its demonstrated metric, data signal,
and event, and drive its custom command verb — all live over an MQTT broker, no hardware needed.

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

```bash
java -jar target/<<JARNAME>>-1.0.0.jar \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/<<COMPONENTNAME>>.json -t tutorial-thing
```

## Step 3 — Watch the heartbeat

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/state' -v
```

A keepalive publishes automatically every heartbeat interval — no code required for this one.

## Step 4 — Watch the demo metric, signal, and event

```bash
mosquitto_sub -h localhost -t 'ecv1/+/+/metric/#' -v &
mosquitto_sub -h localhost -t 'ecv1/+/+/data/#' -v &
mosquitto_sub -h localhost -t 'ecv1/+/+/evt/#' -v &
```

Every tick (10 s by default) you see: a `loopTicks` metric (a monotonic `tickCount` plus an
`uptimeSecs` gauge), a `demo-signal` data message (a sine-wave reading via the `data()` facade), and a
`sample-event` info-level event via the `events()` facade.

## Step 5 — Drive the custom command verb

```bash
mosquitto_pub -h localhost -t 'ecv1/tutorial-thing/<<BINNAME>>/cmd/set-greeting' -m \
  '{"header":{"name":"set-greeting","version":"1.0"},"body":{"greeting":"Hi there"}}'
```

Watch the `app`-status topic (`ecv1/+/+/app/#`) on its next tick — the greeting has changed, proving
the command actually mutated the component's state.

## Step 6 — Clean up

Stop the component with Ctrl-C, and remove the broker:

```bash
docker rm -f emqx
```

## What you did

You ran the scaffold, watched its automatic heartbeat, watched the demonstrated metric/signal/event
trio publish on a timer, and issued a custom command verb that visibly changed component state on the
next tick.

## Next steps

- Replace the demo surface with your own logic: [How-to guides](how-to-guides.md#replace-the-demo-surface-with-your-own-business-logic).
- Understand why identity is config-driven and what each facade owns: [Explanation](explanation.md).
- Look up every topic and payload shape: [Reference — Messaging Interface](reference/messaging-interface.md).
