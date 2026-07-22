# Tutorial — From scaffold to a live demo surface

*This documents the generated scaffold; rewrite it as you build the component out.*

By the end you will have built `<<COMPONENTFULLNAME>>`, run it, and watched its demo
metric/signal/event quartet cross the bus — then commanded it and watched the effect land on the
next tick.

## 1. Prerequisites

- A Rust toolchain (edition 2021, `rust-version = "1.85"`).
- A local MQTT broker on `localhost:1883` (`docker run -d -p 1883:1883 emqx/emqx`).

## 2. Build it

```bash
cargo build
```

## 3. Run it

```bash
cargo run -- \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

Every `publish_interval` seconds (`3` in `test-configs/config.json`) the component ticks: it
publishes an app-status message, emits a metric, publishes a data signal, and emits an event — all
described in the [README](../README.md#the-demonstrated-monitoring--command-surface) and
[reference/messaging-interface.md](reference/messaging-interface.md).

## 4. Watch it

```bash
mosquitto_sub -t 'ecv1/+/+/data/#' -v      # demo-signal, a sine wave
mosquitto_sub -t 'ecv1/+/+/evt/#' -v       # sample-event
mosquitto_sub -t 'ecv1/+/+/metric/#' -v    # loopTicks (only with metricEmission.target: messaging)
mosquitto_sub -t 'ecv1/+/+/+/state' -v     # the automatic keepalive
```

(The default `metricEmission.target` is `log`, which writes to a local file instead of the bus —
set it to `messaging` in `test-configs/config.json` to see `loopTicks` on the wildcard above.)

## 5. Command it

```text
publish ecv1/my-thing/<<COMPONENTNAME>>/cmd/set-greeting
  {"header":{"name":"set-greeting","version":"1.0"},"body":{"greeting":"Hi there"}}
```

The app-status publish on the next tick reflects the new greeting — a command's effect is visibly
observable without a dedicated "get" verb.

## 6. Prove it end-to-end

```bash
cargo test
```

`src/app.rs` unit-tests the custom command handler and the config-driven app construction directly
— no broker required.

Next: the [how-to guides](how-to-guides.md) for replacing the demo with your own metric/signal/
event/command and deploying; the [reference](reference/) for every option and topic; the
[explanation](explanation.md) for why the facades are shaped the way they are.
