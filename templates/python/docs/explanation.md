# Explanation — Why this scaffold looks the way it does

*This documents the generated scaffold; rewrite it as you build the component out.*

This page is the mental model for the **service** archetype — the plainest of the four scaffold
kinds (service, protocol-adapter, processor, sink). For exact options see [reference/](reference/);
for tasks, the [how-to guides](how-to-guides.md).

## What "service" means here

A service component is the archetype with no prescribed data shape: it isn't required to poll a
device (that's a protocol-adapter), transform a stream (a processor), or deliver to an external
destination (a sink). It is the starting point for whatever your business logic actually needs —
which is why the scaffold demonstrates the library's surface rather than imposing one.

## What the library gives you for free

Two things run before `app/<<COMPONENTNAME>>.py` is ever constructed, entirely library-owned:

- **The `state` keepalive** — publishes on `ecv1/{device}/<<BINNAME>>/main/state` roughly every
  5 seconds, carrying `status` (`STARTING`/`RUNNING`/`STOPPING`), uptime, and (when the component
  reports any) `instances[]`.
- **The command inbox** — already answers `ping`, `reload-config`, and `get-configuration` on
  `ecv1/{device}/<<BINNAME>>/main/cmd/#` before `run()` is ever called.

Neither needs a line of code in the scaffold. What the scaffold *adds* is the rest of the surface an
edge-console reads — so a freshly generated component has something to show on the console's
Signals/Events/Metrics tabs and something custom to command, instead of an empty dashboard.

## The four demonstrated facades

| Facade | What it does | Why it exists as a facade, not a raw publish |
|---|---|---|
| `gg.get_metrics()` | Defines and emits `loopTicks` | `MetricBuilder` is the sanctioned construction path — a metric is never built by hand. |
| `gg.data()` | Publishes `demo-signal` | `DataFacade` constructs the `SouthboundSignalUpdate` body, sanitizes the channel, and mints the topic — a component never assembles that body or topic itself. |
| `gg.events()` | Emits `sample-event` | `EventsFacade` derives the `evt/{severity}/{type}` channel from the body's own severity + type, so the topic and body can never disagree. |
| `EdgeCommonsBuilder.configure_commands(...)` | Registers `set-greeting` | Handlers run through the same coded-error contract (`CommandException`) as the built-ins, so a malformed request is never an unhandled crash. |

All four are replaceable, and none is required — a bare scaffold with none of them still runs; they
exist so the demonstrated surface is live end to end out of the box, not because a service component
must have a metric, a signal, an event, and a command.

## One provider, two surfaces (instance connectivity)

`instance_connectivity()` is registered once (`gg.set_instance_connectivity_provider(...)`) and read
from two places: the `state` keepalive pushes whatever it returns into `instances[]` on every tick,
and the built-in `status` command verb returns the very same sample when asked. Whoever *watches*
the bus and whoever *asks* a question can never get different answers, because there is exactly one
source. This scaffold owns no southbound connections, so it reports an empty list — a real answer
("no instances"), not a missing one. The seam is registered anyway so it's visible the day this
component grows a connection of its own (see the [how-to guide](how-to-guides.md#report-a-real-connection)).

## Readiness

The scaffold calls `.initial_ready(False)` on the builder and only calls `gg.set_ready(True)` after
constructing the app — so the library's readiness gate (which additionally requires messaging to be
connected and the command inbox `ACTIVE`) reflects the component actually being ready to work, not
merely having started. The custom handler is installed via `configure_commands(...)` **before** the
inbox subscription is acknowledged, so no request can arrive at a half-registered inbox.

## UNS addressing

Every topic is `ecv1/{device}/{component}/{instance}/{class}[/channel]`, minted by the library's UNS
builder from the config-resolved `hierarchy`/`identity` — never a hand-assembled string. `state`,
`metric`, `cfg`, and `log` are library-owned reserved classes; a component only ever mints `data`,
`evt`, and `app`/`cmd` topics through the facades above. A fleet consumer subscribes one wildcard per
class (`ecv1/+/+/+/data/#`, `.../evt/#`, `.../metric/#`, `.../state`) rather than per-component topic
templates.
