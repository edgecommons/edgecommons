This documents the generated scaffold; rewrite it as you build the component out.

# How-to Guides

Recipes for specific tasks. Each assumes the component builds and runs (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Add your own metric

Define it once (in the constructor, via `MetricBuilder`) and emit it wherever your logic produces a
value — the shipped `loopTicks` metric (`src/app.ts`) is the pattern to copy:

```ts
this.metrics.defineMetric(
  MetricBuilder.create("myMetric").withConfig(this.config)
    .addMeasure("myCount", "Count", 60)
    .build(),
);
// later:
await this.metrics.emitMetric("myMetric", { myCount: n });
```

---

## Publish a data signal

Use the `data()` facade — never hand-build the topic or body:

```ts
await this.data?.publish("my-signal", value); // defaults quality to GOOD, qualityRaw: "unspecified"
// or, with an explicit quality when your source knows a read failed or is stale:
await this.data?.signal("my-signal").addSample(value, { quality: Quality.Bad }).publish();
```

---

## Emit an event

Use the `events()` facade — severity **derives** the channel, so the topic and body can never
disagree:

```ts
await this.events?.emit(Severity.Warning, "my-event", "something happened", { detail: "..." });
// for a STATEFUL condition (raised until explicitly cleared):
await this.events?.raiseAlarm("my-alarm", "something is wrong", { ... });
await this.events?.clearAlarm("my-alarm", { ... });
```

---

## Register a custom command verb

```ts
commands?.register("my-verb", (request: Message) => {
  // validate request.body, throw CommandException(code, message) on bad input
  return { result: "..." }; // becomes { ok: true, result: {...} } in the reply
});
```

`ping` / `reload-config` / `get-configuration` are already live — no need to reimplement them.

---

## Report a connection this component owns

If your component grows a real connection (a database, an upstream API), return it from
`instanceConnectivity()` (`src/app.ts`) instead of an empty array — see the function's own doc
comment for the shape (`InstanceConnectivity.of(id, connected, endpoint).withState(...)
.withAttributes(...)`). One provider feeds both the `state` keepalive's `instances[]` and the
built-in `status` verb, so a console that watches and one that asks can never disagree.

---

## Deploy to a platform

**HOST:** `node dist/main.js --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`

**Greengrass:** package per `gdk-config.json`/`recipe.yaml`; config comes from the deployment
(`--platform GREENGRASS -c GG_CONFIG`).

**Kubernetes:** build the image (`Dockerfile`), apply `k8s/` (config from a mounted ConfigMap,
identity from the Downward API).
