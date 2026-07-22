This documents the generated scaffold; rewrite it as you build the component out.

# Tutorial — From zero to a verified delivery

By the end you'll have `<<COMPONENTNAME>>` consuming messages off the bus and delivering each one
to the local filesystem, watching the full event ladder (started → completed) as it happens. No
external destination required — the shipped `LocalDestination` needs only a writable directory.

## 1. Prerequisites

- Node.js 20+, and a local MQTT broker on `localhost:1883` (`docker run -d -p 1883:1883 emqx/emqx`).
- The sibling `edgecommons` TypeScript library built (`npm run build` in `libs/ts`) — this scaffold
  depends on it via a `file:` path (`--dep-source local`, the default).

## 2. Install, build, and run

```bash
npm install
npm run build
node dist/main.js \
  --platform HOST --transport MQTT ./test-configs/standalone-messaging.json \
  -c FILE ./test-configs/config.json \
  -t my-thing
```

`test-configs/config.json` configures one sink, `archive`, subscribing `ecv1/+/+/+/data/#` and
delivering to `./out`.

## 3. Feed it something

```bash
mosquitto_pub -h localhost -p 1883 -t 'ecv1/my-thing/some-adapter/main/data/temperature-1' -m \
  '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.4,"quality":"GOOD"}]}}'
```

## 4. Watch the delivery ladder

```bash
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/evt/#' -v
```

You'll see `delivery-started` immediately, then `delivery-completed` once the item lands and is
verified. Check the file itself:

```bash
ls ./out/archive/
```

## 5. See a failure and a retry

Point the sink at a path it can't write to (e.g. a file where a directory should be) to see the
other half of the ladder: `delivery-failed` (with `willRetry: true`), repeating with exponential
backoff, until either it succeeds or `retry.giveUpAfterMs` is spent — at which point you'll see the
**Critical** `delivery-exhausted` event. That's the "data did not arrive" signal — the one metric
and event you should always be watching in production.

## 6. Run the tests

```bash
npm test
```

Every suite is self-contained (a temp directory stands in for `./out`) — they exercise the delivery
ladder invariants directly against `src/app.ts`/`src/dest.ts`.

Next: the [how-to guides](how-to-guides.md) for implementing a real destination, tuning retry, and
deploying; the [reference](reference/) for every option; the [explanation](explanation.md) for why
the ladder is ordered the way it is.
