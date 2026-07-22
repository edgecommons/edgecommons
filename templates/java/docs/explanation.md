# Explanation — The Service Archetype

> This documents the generated scaffold; rewrite it as you build the component out.

This page explains the shape of the scaffold so the demo surface and the config make sense as a
whole. For a specific value or procedure, see the [reference](reference/) and the
[how-to guides](how-to-guides.md).

## What the library gives you, automatically

Every EdgeCommons component gets configuration, a messaging transport, metrics, logging, a heartbeat,
and a command inbox with no code: `EdgeCommonsBuilder.create(...)` wires all of it. The `state`
keepalive publishes on its own; `ping`/`reload-config`/`get-configuration` answer on the command inbox
as soon as its transport subscription is acknowledged. A bare scaffold with none of the demo pieces
below still runs, connects, and is visible on the `state` wildcard.

## What this scaffold adds, and why

A component with only the automatic surface has nothing on an edge-console's Signals/Events/Metrics
tabs and nothing custom to command — so this scaffold demonstrates the rest of that surface (see
DESIGN-uns §7/§9) through the **app-usable class facades** rather than hand-built topics/bodies:

| Surface | Facade | Why a facade instead of a raw publish |
|---|---|---|
| Metric | `gg.getMetrics()` / `MetricBuilder` | `MetricBuilder` is the sanctioned construction path; a raw `Metric` constructor is deprecated. |
| Data signal | `gg.getData()` / `DataFacade` | Mints the topic from the signal id, builds the `SouthboundSignalUpdate` body, defaults an omitted quality to `GOOD`. |
| Event | `gg.getEvents()` / `EventsFacade` | Derives the `evt/{severity}/{type}` channel from the body's own severity + type, so the topic and body can never disagree. |
| Command verb | `EdgeCommonsBuilder.configureCommands(...)` | Installs your verb alongside the library's built-ins, before the inbox can go `ACTIVE`. |

## Identity is config-driven

The top-level `hierarchy` (an ordered list of level names) and `identity` (a value for every level
except the last) blocks resolve this component's place in the enterprise hierarchy — the **last**
hierarchy level's value is always the resolved thing name (`-t/--thing`, or the platform's identity
source). Every envelope built `.withConfig(...)` carries that identity automatically, and every topic
is minted through `gg.getUns()` (or, for the demo facades, by the facade itself) — never hand-written.
This is what lets the same component run on HOST, Greengrass, or Kubernetes with the same topics.

## Readiness gating

The scaffold calls `initialReady(false)` and only flips to `setReady(true)` after its metric is
defined and its custom command verb is installed. Readiness additionally requires connected messaging
and an acknowledged command-inbox subscription — so a console or orchestrator that gates traffic on
readiness never sees a component that looks up but cannot yet answer its own commands. Keep this
ordering (define everything, then `setReady(true)`) as you add startup work.

## Why the demo command mutates state instead of just answering

`set-greeting` writes to an in-memory field that the periodic `app`-status publish then reads back on
its next tick. A command that only replies proves the command inbox works; a command whose effect
shows up on a subsequent publish proves the whole loop — command in, state change, observable out —
which is the shape almost every real command has (a mode toggle, a setpoint change, a pause flag).
