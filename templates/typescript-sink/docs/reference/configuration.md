This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Configuration

Every configuration option this scaffold understands. For *why* these exist, see
[../explanation.md](../explanation.md); for tasks, [../how-to-guides.md](../how-to-guides.md).

## Config source

One JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`, `GREENGRASS` →
`GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under `component`
(this document, one **sink** per `component.instances[]` entry); the sibling sections (`tags`,
`hierarchy`, `identity`, `messaging`, `logging`, `metricEmission`, `heartbeat`) are standard
edgecommons sections owned by the canonical schema and are not redeclared here.

## `component.global.defaults`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `retry` | object | see below | Fallback retry policy for a sink that omits its own `retry`. |
| `maxQueue` | number | `256` | Fallback queue bound for a sink that omits its own `maxQueue`. |

## `component.instances[]` (one sink per entry)

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | — | Stable sink id. Prefixes every delivered key (`keyFor`) and appears in logs/events. |
| `subscribe` | string | yes | — | The single topic filter whose messages this sink delivers. |
| `destination` | object | yes | — | A tagged object naming the backend and its arguments (below). |
| `retry` | object | no | `global.defaults.retry` | Per-sink override of the retry policy (below). |
| `maxQueue` | number | no | `global.defaults.maxQueue` | Per-sink override of the queue bound. |

### `destination`

| `type` | Extra keys | Definition |
|--------|-----------|-----------|
| `"local"` | `path` (string, required) | The root directory delivered objects land under. Write-temp-then-rename, so a crash mid-write leaves no corrupt artifact at the final key. |

Add a variant here (and to `src/dest.ts`'s `DestinationConfig`/`buildDestination`) as you implement
a real backend. Whatever the backend, two properties are non-negotiable — see
[../explanation.md](../explanation.md#the-two-non-negotiable-properties).

### `retry`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `baseDelayMs` | number | `1000` | The first backoff window. Each attempt doubles it, up to `maxDelayMs`. |
| `maxDelayMs` | number | `900000` | The backoff ceiling (15 min), so a long outage doesn't back off to next week. |
| `giveUpAfterMs` | number | `3600000` | The **time budget** (1 hour), not an attempt count — see [../explanation.md](../explanation.md#retry-with-full-jitter-and-a-time-budget). When spent, the item is reported `delivery-exhausted`. |

## Unknown keys are rejected

`parseSink` (`src/app.ts`) rejects a sink entry with an unrecognized top-level key (and `retry`
with an unrecognized key) — a typo'd key is a mistake, not a silently-ignored no-op.
