# How-to Guides

*This documents the generated scaffold; rewrite it as you build the component out.*

Recipes for specific tasks. Each assumes the scaffold runs (see the [tutorial](tutorial.md)). For
concepts see [explanation.md](explanation.md); for exhaustive options see [reference/](reference/).

---

## Replace the demo metric

`app/<<COMPONENTNAME>>.py` defines `loopTicks` once in `__init__` (`MetricBuilder.create(...)`) and
emits it every loop in `run()`. Replace the two measures (`tickCount`, `uptimeSecs`) with your own,
and add a real `add_dimension(...)` if a dimension helps you slice it (keep dimensions
low-cardinality — an instance id or a result code, never a raw value or an id that grows without
bound).

```python
self._metrics.define_metric(
    MetricBuilder.create("myMetric")
    .with_config(self._config_manager)
    .add_measure("itemsProcessed", "Count", 60)
    .build()
)
# later, in run():
self._metrics.emit_metric("myMetric", {"itemsProcessed": float(n)})
```

## Replace the demo data signal

The `data()` facade (`gg.data()`, or `gg.instance(id).data()` once you have real instances) mints
the topic and stamps identity — never hand-build a `SouthboundSignalUpdate` or a topic string. Map
one real reading onto one call:

```python
self._data.publish("real-signal-id", reading_value)
```

Pass an explicit `Quality.BAD`/`Quality.UNCERTAIN` when your source knows a read failed or is stale
— an omitted quality defaults to `GOOD` (marked `qualityRaw: "unspecified"` on the wire), which is
right for a synthesized demo value but wrong for a device you can't currently reach.

## Replace the demo event

`events().emit(type, message, context, severity=...)` derives the `evt/{severity}/{type}` channel
from the body's own severity and type, so the topic and the body can never disagree. Emit these on
real occurrences (a threshold crossed, a connection lost/restored), not on a timer. Use
`raise_alarm`/`clear_alarm` instead of `emit` for a **stateful** condition — something that stays
true until explicitly cleared, like a connection being down.

## Add your own command verb

Register it on the builder, **before** `gg.set_ready(True)`, alongside the automatic built-ins:

```python
gg = (
    EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
    .with_args(sys.argv[1:])
    .configure_commands(lambda inbox: inbox.register("my-verb", my_handler))
    .build()
)
```

The inbox **rejects** a verb name that collides with a built-in (`ping`, `status`, `describe`,
`reload-config`, `get-configuration`) rather than silently shadowing it — pick a distinct name. A
handler that hits a malformed argument should raise `CommandException("BAD_ARGS", "...")` (or
another coded error), never let an unhandled exception escape — see `GreetingState.handle` for the
pattern.

## Report a real connection

Once the component owns a southbound connection (a device, a database, an upstream API), replace
`instance_connectivity()`'s empty list with one entry per connection:

```python
from edgecommons.heartbeat.instance_connectivity import InstanceConnectivity

def instance_connectivity(self):
    return [
        InstanceConnectivity.of("plc-1", client.is_connected(), "opc.tcp://plc-1:4840")
        .with_state("ONLINE")
        .with_attributes({"sessionId": client.session_id})
    ]
```

This is read by **two surfaces that must never disagree**: the `state` keepalive pushes it into
`instances[]` on every tick, and the built-in `status` verb returns the same sample when asked.
Keep it cheap and non-blocking — it is sampled on every heartbeat tick.

## Deploy to a platform

**HOST:** `python3 main.py --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`
(or `docker compose up --build`, which also starts a local EMQX broker).

**Greengrass:** package with the GDK (`gdk component build && gdk component publish`), using
`gdk-config.json`/`recipe.yaml`. Config comes from the deployment (`--platform GREENGRASS -c GG_CONFIG`).
Set a real S3 bucket in `gdk-config.json` before publishing — a scaffold with no bucket configured
carries a visible sentinel that `component validate` treats as an error.

**Kubernetes:** build `./Dockerfile`, push or `kind load` it, set `image:` in `k8s/deployment.yaml`,
then `kubectl apply -f k8s/`. With `--platform auto` the library detects KUBERNETES from the
ServiceAccount token, reads config from the mounted ConfigMap (hot-reloaded on `kubectl apply`),
and resolves identity from the Downward API — the Deployment needs no CLI args.

## Wire up CI

`.github/workflows/ci.yml` calls the org's reusable component-CI workflow plus a `coverage` job
enforcing the 90% line-coverage gate; `.github/workflows/deploy-docs.yml` refreshes the docs site on
doc-only pushes once this component is registered in `registry/components.json`. Both are inert
until the repo is pushed to GitHub with the org secrets configured — nothing to do locally beyond
pushing the repo.
