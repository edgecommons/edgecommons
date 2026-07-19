This documents the generated scaffold; rewrite it as you build the component out.

# Explanation — What this scaffold demonstrates, and why

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## Two things every component gets for free

Before any code in `src/app.ts` runs, the library has already wired a **`state` keepalive**
(publishing on / 5s / local by default) and a **command inbox** answering `ping`,
`reload-config`, and `get-configuration`. This scaffold's constructor adds to that surface; it does
not replace or duplicate it.

## Why a demo surface at all

A bare scaffold works with none of `src/app.ts`'s content — the library alone is a runnable
component. The demo metric/signal/event/command exist so a freshly generated component has
something to show on an edge-console's Signals/Events/Metrics tabs and something to command,
instead of an empty dashboard. Delete all four once you have real business logic to show instead;
none of them are required.

## Facades over hand-built topics

`gg.data()`, `gg.events()`, and `gg.commands()` exist so application code never mints a UNS topic
or assembles an envelope body by hand. `DataFacade` constructs the `SouthboundSignalUpdate` shape
and defaults an omitted sample quality to `GOOD` (marked `qualityRaw: "unspecified"` on the wire, so
a consumer can tell a synthesized reading from a device-reported one). `EventsFacade` derives the
`evt/{severity}/{type}` channel from the event's own severity and type, so the topic and the body
can never disagree — there is no way to publish an event whose channel contradicts what's inside
it. Both facades throw if no messaging transport is wired (e.g. GREENGRASS with no IPC configured);
`App`'s constructor guards each with try/catch and degrades to heartbeat-only rather than crashing.

## One provider, two surfaces: instance connectivity

`gg.setInstanceConnectivityProvider(...)` registers one function that the library samples from two
different places: the `state` keepalive's `instances[]` array (pushed on every tick) and the
built-in `status` verb (pulled on demand). Because both read the *same* function, a console that
subscribes to the keepalive and one that asks via a command can never get different answers about
whether a connection is up. This scaffold reports **no** connections (`instanceConnectivity()`
returns `[]`) — the honest answer for a component that owns none — but the function's own doc
comment shows the shape to return once you add one.

## Identity, and why the last hierarchy level is always the device

Every topic is minted from the config's `hierarchy`/`identity` blocks through `gg.uns()`. The
**last** hierarchy level's value is always the resolved Thing name (the device) — every other level
comes from `identity`. This is what lets a fleet consumer subscribe one wildcard per UNS class
(`ecv1/+/+/+/data/#`, …) instead of learning a per-site topic template: the enterprise location
rides the message's `identity` element, not a bespoke topic shape.

## Dynamic config pickup

The `ConfigurationChangeListener` registered in the constructor is what lets a deployment/shadow
config change take effect without a restart — returning `true` from
`onConfigurationChange` tells the library the component accepted the new config. This scaffold's
listener just logs; a real component re-reads whatever config values it cares about and applies
them.
