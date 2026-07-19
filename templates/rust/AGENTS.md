# <<COMPONENTNAME>> — component notes

EdgeCommons **general-purpose component** (Rust). Full name `<<COMPONENTFULLNAME>>`, crate/binary
`<<BINNAME>>`. Depends on the `edgecommons` Rust library. If this repo lives inside the EdgeCommons
org umbrella workspace, read its root `AGENTS.md` first (org repo map, design-fidelity contract,
validation matrix, platform/transport model); everything below is this component's own detail.

## What it is

A minimal EdgeCommons component: the library's standard CLI contract, configuration, logging,
messaging, metrics, and heartbeat, plus a demonstrated monitoring/command surface (a periodic
metric, data signal, event, and a custom command verb) so a freshly generated component has
something live to observe and command. Runs on `GREENGRASS` / `HOST` / `KUBERNETES` via
`edgecommons` — no platform branching in this component's own code.

## The seam

There is no fixed archetype seam here — `src/app.rs`'s `App` is the whole of this component's logic,
built directly against the library's facades (`gg.data()`, `gg.events()`, `gg.metrics()`,
`gg.commands()`). If this component grows a device connection, a delivery destination, or a
subscribe-transform-forward pipeline, consider the matching archetype template
(`protocol-adapter`/`sink`/`processor`) instead of building that shape from scratch here.

## Config location

This component's own settings live under `component.global` / `component.instances[]` in the
EdgeCommons config document (`config.schema.json` is the contract); the sibling sections (`tags`,
`hierarchy`, `identity`, `messaging`, `metricEmission`, `logging`, `heartbeat`) are the standard
`edgecommons` envelope, owned by the canonical schema and not redeclared here. `test-configs/`
carries a runnable example.

## Validation expectations

- `cargo test` covers the custom command handler and config-driven app construction directly — no
  broker required.
- `cargo llvm-cov --fail-under-lines 90` is the coverage gate (`.github/workflows/ci.yml`'s
  `coverage` job) — the org rule is 90% line coverage per language. Do not lower the gate or exclude
  testable code to pass it.
- `edgecommons component validate` checks this repo's config against `config.schema.json` and warns
  if `Cargo.lock` is not committed.

## Org conventions this scaffold inherits

- Builders/facades are the construction path (`data()`, `events()`, `commands()`, `MetricBuilder`) —
  never hand-built topics or envelopes.
- The instance-connectivity provider is registered even with nothing to report — the seam should be
  visible before it is needed, not added retroactively.
- Four-way parity: if this repo's Java/Python/TypeScript siblings exist, observable behavior should
  match — same config shape, same command verbs, same metric names.
- Runtime artifacts (vaults, parameter caches, generated streams, TLS certs, logs, build output,
  local broker state) stay out of Git.
