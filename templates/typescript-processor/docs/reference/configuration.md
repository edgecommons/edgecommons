This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Configuration

Every configuration option this scaffold understands. For *why* these exist, see
[../explanation.md](../explanation.md); for tasks, [../how-to-guides.md](../how-to-guides.md).

## Config source

One JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`, `GREENGRASS` →
`GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under `component`
(this document, one **route** per `component.instances[]` entry); the sibling sections (`tags`,
`hierarchy`, `identity`, `messaging`, `logging`, `metricEmission`, `heartbeat`) are standard
edgecommons sections owned by the canonical schema and are not redeclared here.

## `component.token`

The UNS component token: the `{component}` segment of every topic this component publishes on, and
the `identity.component` field of every message envelope. UNS tokens are lower-kebab
(`<<BINNAME>>`); the Greengrass component name is the reverse-DNS `<<COMPONENTFULLNAME>>` and never
reaches the wire. Every shipped configuration and the recipe set it. Leave it set — without it the
library falls back to the short form of the component name, `<<COMPONENTNAME>>`, and the topics in
this documentation stop matching what the component publishes.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `tickMs` | number | `10000` | Fallback tick cadence for a route that omits its own `tickMs`. |
| `maxQueue` | number | `256` | Fallback queue bound for a route that omits its own `maxQueue`. |

## `component.instances[]` (one route per entry)

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | — | Stable route id. Appears in logs and is the label a metrics dashboard groups on. |
| `subscribe` | string[] | no | `[]` | Topic filters this route consumes. Wildcards allowed (e.g. `ecv1/+/+/+/data/#`). |
| `publishTopic` | string | yes | — | Where the transformed result publishes. Resolved through the library's config-template resolver — `{ThingName}`, `{ComponentName}`, a hierarchy level, or a tag key may be interpolated, so one deployed config addresses the device it actually landed on. |
| `target` | `"local"` \| `"northbound"` | no | `"local"` | Where the result goes: the device-local bus, or straight to the northbound broker. |
| `pipeline` | array | no | `[]` | The stages, in order (below). An empty pipeline is a pass-through republisher. |
| `maxQueue` | number | no | `global.defaults.maxQueue` | Per-route override of the queue bound. |
| `tickMs` | number | no | `global.defaults.tickMs` | Per-route override of the stage tick cadence. |

## Stages (`pipeline[]` entries)

A single-key object naming the stage and its arguments.

| Stage | Arguments | Behavior |
|---|---|---|
| `fieldEquals` | `path` (string), `value` (any) | Keeps only messages whose dotted body path equals `value`; drops the rest. |
| `countPerTick` | (none) | Accumulates arrivals; emits `{count, last}` once per `tickMs`. |

Add your own stage's arguments here as you implement it in `src/proc.ts` — see
[../how-to-guides.md](../how-to-guides.md#write-your-own-stage).

## Unknown keys are rejected

`parseRoute` (`src/app.ts`) rejects a route entry with an unrecognized top-level key, and
`buildStage` (`src/proc.ts`) rejects a stage naming anything other than exactly one known stage
kind — a typo'd key is a mistake, not a silently-ignored no-op.
