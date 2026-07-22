# How-to Guides

*This documents the generated scaffold; rewrite it as you build the component out.*

Recipes for specific tasks. Each assumes the adapter runs against the simulator (see the
[tutorial](tutorial.md)). For concepts see [explanation.md](explanation.md); for exhaustive options
see [reference/](reference/).

---

## Implement your protocol

Implement `DeviceBackend`/`DeviceSession` (`<<SNAKENAME>>/device.py`) once per protocol — everything
above the seam (the connect/poll/reconnect worker, the command surface, the metrics) is written
against the abstraction and never learns your protocol:

```python
class MyBackend(DeviceBackend):
    def kind(self) -> str:
        return "myproto"

    def inventory(self, connection: dict) -> list:
        return [SignalInfo(id=sid, name=name) for sid, name in my_configured_signals(connection)]

    def connect(self, connection: dict) -> "DeviceSession":
        try:
            client = my_client_lib.connect(connection["endpoint"])
        except my_client_lib.ConnectionError as e:
            raise DeviceError(str(e), transient=True)   # or transient=False for a config error
        return MySession(client)


class MySession(DeviceSession):
    def read_signals(self) -> list:
        # A per-signal failure returns Quality.BAD for THAT signal; only raise DeviceError when the
        # connection itself is broken.
        ...

    def write_signal(self, signal_id: str, value) -> None:
        ...
```

Register it in `make_backend()` (`device.py`'s bottom), matching the `adapter` config key:

```python
def make_backend(adapter: str):
    if adapter == "sim":
        return SimBackend()
    if adapter == "myproto":
        return MyBackend()
    return None
```

Override `browse()` if your protocol has discovery (OPC UA browse, an EtherNet/IP tag list); the
default raises `BrowseUnsupported`, which `sb/browse` maps to the honest `BROWSE_UNSUPPORTED` error
rather than pretending to enumerate nothing.

## Add a second device

Each entry of `component.instances[]` is one device — its own worker thread, its own connection, its
own write allow-list:

```jsonc
{
  "id": "device-2",
  "adapter": "myproto",
  "connection": { "endpoint": "myproto://10.0.0.51:9000" },
  "pollIntervalMs": 2000,
  "writes": { "allow": ["setpoint-1"] }
}
```

One device going offline never disturbs another — each has its own connect/backoff loop. With more
than one device configured, every `sb/*` command **requires** `instance` in its body (see
[explanation.md](explanation.md#instance-routing)).

## Allow a signal to be written

Add its stable `signal.id` to that device's `writes.allow` array. An empty list (the default) means
the device is read-only — the correct default for anything touching a control system. The allow-list
is checked **before any device I/O**, so a refused write never reaches your `write_signal()`.

## Read/write from a client

Both go through the library **command inbox** (`ecv1/{device}/<<BINNAME>>/cmd/{verb}`). See the
[tutorial](tutorial.md#5-read-a-signal-on-demand) for worked requests, and
[reference/messaging-interface.md](reference/messaging-interface.md) for every payload shape and
error code.

## Tune the poll cadence

`component.global.defaults.pollIntervalMs` sets the fallback; a device's own `pollIntervalMs`
overrides it. Lower = fresher data, more protocol traffic; there is no coalescing built in (unlike
the Modbus reference adapter) — add it in your `DeviceSession` if your protocol benefits from
batching reads.

## Tune staleness detection

`component.global.healthThresholds.staleSignalSecs` (default `30`) sets how long a signal can go
without an update before it counts toward `southbound_health.staleSignals`. Lower it for a fast poll
loop where a stalled signal should alarm quickly; raise it for a slow one where normal cadence looks
"stale" by a naive short threshold.

## Add your protocol's metric families

`southbound_health` plus `<<COMPONENTNAME>>Connection`/`<<COMPONENTNAME>>Command` ship as the
canonical floor. Add `<<COMPONENTNAME>>Inventory`/`Poll`/`Publish` families for protocol-specific
detail (poll cycles, samples good/bad, batch flushes, …) — see `<<SNAKENAME>>/metrics.py`'s module
docstring for the extension point and `modbus-adapter/modbus_adapter/metrics.py` for the full worked
set. Register each new family in `family_defs()` and `DeviceMetrics.define_all()`; keep dimensions
low-cardinality (`instance`, a bounded category — never a signal name, address, or endpoint URL).

## Run the live-sim integration test

`tests/test_live_sim.py` is skipped by default (`@pytest.mark.skipif`). Point it at a running
simulator or real device and set `EC_LIVE_SIM=<endpoint>` to run it:

```bash
EC_LIVE_SIM=sim://device-1 python -m pytest tests/test_live_sim.py -v
```

It connects, runs one poll cycle, and asserts readings + quality — the same shape the reference
adapters' live-infra suites use (see the module docstring for how they wire theirs against a real
simulator container).

## Deploy to a platform

**HOST:** `python main.py --platform HOST --transport MQTT ./messaging.json -c FILE ./config.json -t my-thing`
(or `docker compose up --build`).

**Greengrass:** `gdk component build && gdk component publish`, using `gdk-config.json`/`recipe.yaml`.
Config comes from the deployment (`--platform GREENGRASS -c GG_CONFIG`). Set a real S3 bucket in
`gdk-config.json` before publishing — a scaffold with no bucket configured carries a visible sentinel
that `component validate` treats as an error.

**Kubernetes:** build `./Dockerfile`, push or `kind load` it, set `image:` in `k8s/deployment.yaml`,
then `kubectl apply -f k8s/`. With `--platform auto` the library detects KUBERNETES, reads config
from the mounted ConfigMap (hot-reloaded on `kubectl apply`), and resolves identity from the Downward
API.

## Wire up CI

`.github/workflows/ci.yml` calls the org's reusable component-CI workflow plus a `coverage` job
enforcing the 90% line-coverage gate; `.github/workflows/deploy-docs.yml` refreshes the docs site on
doc-only pushes once this component is registered in `registry/components.json`. Both are inert until
pushed to GitHub with the org secrets configured.
