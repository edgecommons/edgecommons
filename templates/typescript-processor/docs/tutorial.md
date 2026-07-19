This documents the generated scaffold; rewrite it as you build the component out.

# Tutorial — From zero to a live rollup

By the end you'll have `<<COMPONENTNAME>>` consuming messages from the bus, running them through
its shipped pipeline, and publishing a rollup you can watch appear on the UNS. No external
component required — you can feed it with any MQTT publish.

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

`test-configs/config.json` configures one route, `rollup`: it subscribes `ecv1/+/+/+/data/#`, keeps
only messages whose `signal.id` equals `temperature-1` (the `fieldEquals` stage), counts them, and
publishes a rollup every 10 seconds (`countPerTick`) to `ecv1/gw-01/<<BINNAME>>/rollup/app/summary`.

## 3. Feed it something

Any message on `ecv1/+/+/+/data/#` matches the subscription; only ones whose `signal.id` is
`temperature-1` survive the filter. Publish one directly to try it:

```bash
mosquitto_pub -h localhost -p 1883 -t 'ecv1/my-thing/some-adapter/main/data/temperature-1' -m \
  '{"header":{"name":"SouthboundSignalUpdate","version":"1.0"},"body":{"signal":{"id":"temperature-1"},"samples":[{"value":21.4,"quality":"GOOD"}]}}'
```

Publish a few of these within the same 10-second window.

## 4. Watch the rollup appear

```bash
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/+/app/#' -v
```

On the next tick you'll see `{"count": N, "last": {...}}` — `countPerTick` accumulated on arrival
and emitted on the timer, carrying the last matching message's body.

## 5. Run the tests

```bash
npm test
```

Every suite is self-contained (no broker needed) — they exercise the pipeline invariants (the
self-echo guard, the identity restamp, the bounded queue) directly against `src/app.ts`/`src/proc.ts`.

Next: the [how-to guides](how-to-guides.md) for writing your own stage, adding a route, and
deploying; the [reference](reference/) for every option; the [explanation](explanation.md) for why
the archetype is shaped this way.
