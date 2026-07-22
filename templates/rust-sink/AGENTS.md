# <<COMPONENTNAME>> — component notes

EdgeCommons **sink component** (Rust). Full name `<<COMPONENTFULLNAME>>`, crate/binary
`<<BINNAME>>`. Depends on the `edgecommons` Rust library. If this repo lives inside the EdgeCommons
org umbrella workspace, read its root `AGENTS.md` first (org repo map, design-fidelity contract,
validation matrix, platform/transport model); everything below is this component's own detail.

## What it is

The last thing standing between data on the bus and its destination: it consumes messages, delivers
each one outward, verifies what landed, and only then lets go of the source. Ships with a
`LocalDestination` (filesystem) backend so it runs with no external dependency. Runs on
`GREENGRASS` / `HOST` / `KUBERNETES` via `edgecommons` — no platform branching in this component's
own code.

## The seam

`src/dest.rs`'s `Destination` trait is the one place backend knowledge lives: `deliver` lands an item
at a stable, deterministic key; `verify` confirms it landed correctly before the caller releases the
source. Everything above it (`src/supervisor.rs`'s per-sink task, the retry/backoff ladder, and the
event/metric surface; `src/app.rs`'s retry policy, stable key, and connectivity reporting) is written
against the trait and does not change when a new backend is added.

## Config location

This component's own settings live under `component.global` / `component.instances[]` (one
destination per instance) in the EdgeCommons config document (`config.schema.json` is the contract);
the sibling sections (`tags`, `hierarchy`, `identity`, `messaging`, `metricEmission`, `logging`,
`heartbeat`) are the standard `edgecommons` envelope, owned by the canonical schema and not
redeclared here. `test-configs/` carries a runnable example.

## Validation expectations

- `cargo test` covers the destination contract (`src/dest.rs`) and sink config/retry/connectivity
  (`src/app.rs`) directly — no broker or real backend required.
- `cargo llvm-cov --fail-under-lines 90` is the coverage gate (`.github/workflows/ci.yml`'s
  `coverage` job) — the org rule is 90% line coverage per language. The `ethernet-ip-adapter`
  discipline is followed: the untestable live drivers are isolated in a thin `src/supervisor.rs`
  seam (consume → deliver/verify → retry), and the coverage job passes
  `--ignore-filename-regex '(supervisor\.rs|main\.rs)'` so ONLY that seam and the binary shim are
  excluded — each pinned to a reason in the workflow. Every pure decision they compose (retry
  backoff, the give-up budget, the stable key, connectivity, config defaults, the destination
  contract) stays in `app.rs` / `dest.rs`, in the denominator, and is unit-tested. A real
  destination's live-infra paths (network calls to a real object store) are validated through
  lab/HOST smoke. Do not lower the gate or exclude testable code to pass it — add tests.
- `edgecommons component validate` checks this repo's config against `config.schema.json` and warns
  if `Cargo.lock` is not committed.

## Org conventions this scaffold inherits

- Delivery is idempotent to a stable key, and is verified before the source is released — both are
  load-bearing, not optional style.
- Failure is classified transient vs. permanent; give-up is a time budget, not an attempt count; the
  event ladder (`delivery-started`/`completed`/`failed`/`exhausted`) is reported end to end.
- A sink's destinations are its instances — one connectivity entry per configured sink.
- Four-way parity: if this repo's Java/Python/TypeScript siblings exist, observable behavior should
  match — same config shape, same event types, same metric names.
- Builders/facades are the construction path (`messaging()`, `events()`, `MetricBuilder`) — never
  hand-built topics or envelopes.
- Runtime artifacts (vaults, parameter caches, generated streams, TLS certs, logs, build output,
  local broker state) stay out of Git.
