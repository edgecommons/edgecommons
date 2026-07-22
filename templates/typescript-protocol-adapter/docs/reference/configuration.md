This documents the generated scaffold; rewrite it as you build the component out.

# Reference — Configuration

Every configuration option this scaffold understands. For *why* these exist, see
[../explanation.md](../explanation.md); for tasks, [../how-to-guides.md](../how-to-guides.md); for
what a signal reading's `value`/`quality` mean, [data-types.md](data-types.md).

## Config source

One JSON document from `-c/--config`, defaulting by platform: `HOST` → `FILE`, `GREENGRASS` →
`GG_CONFIG`, `KUBERNETES` → `CONFIGMAP`. This component's own settings live under `component`
(this document); the sibling sections (`tags`, `hierarchy`, `identity`, `messaging`, `logging`,
`metricEmission`, `heartbeat`) are standard edgecommons sections owned by the canonical schema
(`schema/edgecommons-config-schema.json`) and are not redeclared here.

## `component.global`

| Key | Type | Default | Definition |
|-----|------|---------|-----------|
| `healthThresholds.staleSignalSecs` | number | `30` | A signal with no update for longer than this counts toward `southbound_health.staleSignals`. |

`component.global.defaults` is open (`additionalProperties: true`) for your own per-protocol
defaults — the scaffold reads only `pollIntervalMs` as a fallback per instance; extend
`config.schema.json` as you add configuration.

## `component.instances[]`

| Key | Type | Required | Default | Definition |
|-----|------|----------|---------|-----------|
| `id` | string | yes | — | Stable instance id. The `{instance}` token of this device's topics and the `instance` field every `sb/*` command resolves against. |
| `adapter` | string | no | `"sim"` | Which backend `backendFor()` resolves. Add your protocol's kind as you implement it. |
| `connection` | object | yes | — | Deliberately open (`additionalProperties: true`) — every protocol needs different keys. `endpoint` is the one field every backend can rely on. |
| `pollIntervalMs` | number | no | `5000` | How often the device loop reads and publishes, in milliseconds. |
| `writes.allow` | string[] | no | `[]` | The per-instance write allow-list, by stable `signal.id`. Checked **before** any device I/O. Empty ⇒ read-only. |

### `connection`

The only field this scaffold's `App` reads is `endpoint` (published in `device.endpoint` and used
by the simulator's connect check). Everything else is protocol-specific — add the keys your
backend needs (a unit id, a security policy, a slave address) and read them in your
`DeviceBackend.connect()`.

## Unknown keys are rejected

`parseDevice` (`src/app.ts`) rejects an instance entry with an unrecognized top-level key — a
typo'd key is a mistake, not a silently-ignored no-op. `connection` is the one exception: its
contents are never validated against a closed key set, because they vary per protocol.
