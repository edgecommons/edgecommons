This documents the generated scaffold; rewrite it as you build the component out.

# Tutorial — From zero to a live demo surface

By the end you'll have `<<COMPONENTNAME>>` running, publishing a demo metric/signal/event on a
timer, and you'll have invoked its one custom command and watched the effect land. No external
dependency beyond a local MQTT broker.

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

## 3. Watch the demo surface

```bash
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/metric/#' -v   # loopTicks (tickCount, uptimeSecs)
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/data/#' -v     # demo-signal (a sine wave)
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/evt/#' -v      # sample-event (severity Info)
mosquitto_sub -h localhost -p 1883 -t 'ecv1/+/+/state' -v      # the keepalive
```

Each fires every 10 seconds (`TICK_INTERVAL_MS` in `src/app.ts`). Note the topics have no
`{instance}` segment here — this scaffold's demo facades bind to the component's default `main`
instance implicitly.

## 4. Invoke the custom command

```
publish ecv1/my-thing/<<COMPONENTNAME>>/cmd/set-greeting
  {"header":{"name":"set-greeting","reply_to":"app/r","correlation_id":"1"},"body":{"greeting":"Hi there"}}
subscribe app/r → {"ok":true,"result":{"previousGreeting":"Hello from <<COMPONENTNAME>>","greeting":"Hi there"}}
```

The next `app` status publish reflects the new greeting — invoking a command from a console has a
visible, on-the-wire effect.

## 5. Try the built-ins

`ping`, `reload-config`, and `get-configuration` are live with zero code (the library's inbox):

```
publish ecv1/my-thing/<<COMPONENTNAME>>/cmd/ping
  {"header":{"name":"ping","reply_to":"app/r","correlation_id":"2"},"body":{}}
subscribe app/r → {"ok":true,"result":{"status":"RUNNING","uptimeSecs":42}}
```

## 6. Run the tests

```bash
npm test
```

Next: the [how-to guides](how-to-guides.md) for adding your own metric/signal/event/command and
deploying; the [reference](reference/) for every option; the [explanation](explanation.md) for the
facades behind the demo surface.
