# Tutorial — From scaffold to a live rollup

*This documents the generated scaffold; rewrite it as you build the component out.*

By the end you will have built `<<COMPONENTFULLNAME>>`, run it against a message it subscribes to,
and watched a transformed rollup appear on the bus. No external protocol/device dependency required
— the scaffold ships two worked stages that operate on ordinary UNS messages.

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

`test-configs/config.json` configures one route, `rollup`: it subscribes to every `data` message on
the bus (`ecv1/+/+/+/data/#`), keeps only messages whose `signal.id` equals `temperature-1`, counts
them, and republishes a rollup every `tickMs` (10 seconds by default).

## 4. Feed it a message

Publish something the route's filter will keep:

```bash
mosquitto_pub -t 'ecv1/my-thing/some-source/main/data/temperature-1' \
  -m '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.4}]}}'
```

Publish it a few times in a row (each one increments the route's internal counter but emits
nothing yet — `CountPerTick` accumulates on arrival and only emits on its tick).

## 5. Watch the rollup appear

```bash
mosquitto_sub -t 'ecv1/gw-01/<<BINNAME>>/rollup/data/#' -v
```

On the next tick you should see one message whose body is
`{"count": N, "last": {...the last matching message's body...}}` — `N` being however many matching
messages arrived since the previous tick. Publish a message whose `signal.id` is **not**
`temperature-1` and confirm it is dropped silently (the `FieldEquals` stage keeps only what matches).

## 6. Prove it end-to-end

```bash
cargo test
```

`src/proc.rs` unit-tests the pipeline mechanics directly (a filter drops what does not match, a
stateful stage emits on the tick and not on arrival, stages chain correctly); `src/app.rs` unit-tests
config parsing, defaults resolution, and the self-echo guard.

Next: the [how-to guides](how-to-guides.md) for writing your own stage and adding routes; the
[reference](reference/) for every option, topic, and metric; the [explanation](explanation.md) for
why the pipeline is shaped the way it is.
