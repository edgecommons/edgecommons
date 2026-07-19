# Tutorial — From scaffold to a delivered file

*This documents the generated scaffold; rewrite it as you build the component out.*

By the end you will have built `<<COMPONENTFULLNAME>>`, run it against a message it subscribes to,
and found the delivered file on disk. No external backend required — the scaffold ships a
**local filesystem destination** (`LocalDestination`, in `src/dest.rs`) for exactly this reason.

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

`test-configs/config.json` configures one sink, `archive`, subscribing to every `data` message on
the bus and delivering each one under `./out`.

## 4. Send it something to deliver

```bash
mosquitto_pub -t 'ecv1/my-thing/some-source/main/data/temperature-1' \
  -m '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.4}]}}'
```

## 5. Find the delivered object

```bash
ls out/archive/
cat out/archive/temperature-1.*.json
```

The key is deterministic — built from the sink id, the topic's last segment, and the message's own
UUID (`key_for` in `src/dest.rs`) — so publishing the *same* message again **overwrites** the same
file rather than creating a second one. This is what makes the sink's retry safe: a redelivery is an
idempotent overwrite, never a duplicate.

## 6. Watch the event ladder

```bash
mosquitto_sub -t 'ecv1/+/+/+/evt/#' -v
```

Publish another message and you should see, in order:
`delivery-started` (info) → `delivery-completed` (info, carrying `attempts` and `elapsedMs`). The
local destination essentially never fails transiently, so you will not see `delivery-failed`/
`delivery-exhausted` unless you make the destination directory unwritable — try
`chmod 000 out` (or the Windows equivalent) and re-publish to see the retry ladder in action.

## 7. Check connectivity

```text
publish ecv1/my-thing/<<BINNAME>>/cmd/sb/status  (or whatever built-in status verb applies)
```

Or subscribe `ecv1/+/+/+/state` — the keepalive's `instances[]` array carries one entry for
`archive`: `{ "instance": "archive", "connected": true, "state": "IDLE"|"ONLINE", "detail": "./out",
"attributes": { "destination": "local" } }`. An **untried** destination reports `IDLE` (reachable,
just unused) — not a broken one.

## 8. Prove it end-to-end

```bash
cargo test
```

`src/dest.rs` unit-tests the destination contract directly (delivery lands at the stable key,
redelivery overwrites, no partial file is left behind, verify refuses a size mismatch); `src/app.rs`
unit-tests config parsing, defaults, backoff math, and connectivity reporting.

Next: the [how-to guides](how-to-guides.md) for writing a real destination backend and tuning retry;
the [reference](reference/) for every option, topic, and metric; the
[explanation](explanation.md) for why delivery is ordered the way it is.
