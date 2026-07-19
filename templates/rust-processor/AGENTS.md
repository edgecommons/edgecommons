# <<COMPONENTNAME>> — component notes

EdgeCommons **processing component** (Rust). Full name `<<COMPONENTFULLNAME>>`, crate/binary
`<<BINNAME>>`. Depends on the `edgecommons` Rust library. If this repo lives inside the EdgeCommons
org umbrella workspace, read its root `AGENTS.md` first (org repo map, design-fidelity contract,
validation matrix, platform/transport model); everything below is this component's own detail.

## What it is

Subscribes to messages already on the bus, transforms them through an ordered pipeline of stages,
and forwards the result — to the local bus or northbound. Ships with two worked stages
(`FieldEquals`, a filter; `CountPerTick`, a rollup) so it runs and produces output with no external
dependency. Runs on `GREENGRASS` / `HOST` / `KUBERNETES` via `edgecommons` — no platform branching in
this component's own code.

## The seam

`src/proc.rs`'s `Processor` trait is the one place transform logic lives: `process` handles an
inbound message and returns zero or more; `on_tick` lets a stateful stage emit on a timer instead of
on arrival. Everything above it (`src/supervisor.rs`'s per-route task, subscribe/dispatch, and
identity restamp; `src/app.rs`'s self-echo guard, route config, and stage construction) is written
against the trait and does not change when a new stage is added.

## Config location

This component's own settings live under `component.global` / `component.instances[]` (one route per
instance) in the EdgeCommons config document (`config.schema.json` is the contract); the sibling
sections (`tags`, `hierarchy`, `identity`, `messaging`, `metricEmission`, `logging`, `heartbeat`) are
the standard `edgecommons` envelope, owned by the canonical schema and not redeclared here.
`test-configs/` carries a runnable example.

## Validation expectations

- `cargo test` covers the pipeline mechanics (`src/proc.rs`) and the route config, self-echo guard,
  stage construction, and config defaults (`src/app.rs`) directly — no broker required.
- `cargo llvm-cov --fail-under-lines 90` is the coverage gate (`.github/workflows/ci.yml`'s
  `coverage` job) — the org rule is 90% line coverage per language. The `ethernet-ip-adapter`
  discipline is followed: the untestable live drivers are isolated in a thin `src/supervisor.rs`
  seam (subscribe → per-route select-loop → publish), and the coverage job passes
  `--ignore-filename-regex '(supervisor\.rs|main\.rs)'` so ONLY that seam and the binary shim are
  excluded — each pinned to a reason in the workflow. Every pure decision they compose (the
  self-echo guard, config defaults, stage construction, the pipeline mechanic) stays in `app.rs` /
  `proc.rs`, in the denominator, and is unit-tested. Do not lower the gate or exclude testable code
  to pass it — add tests.
- `edgecommons component validate` checks this repo's config against `config.schema.json` and warns
  if `Cargo.lock` is not committed.

## Org conventions this scaffold inherits

- A processor is **payload-agnostic**: it uses raw `messaging()`, never the `data()` facade (which
  imposes the `SouthboundSignalUpdate` shape and mints its own topic from a signal id).
- Self-echo guard + identity restamp are load-bearing, not optional style — removing either breaks
  the fleet's ability to trust who published a message, and the self-echo guard specifically prevents
  an infinite republish loop.
- A full route queue drops and counts; it never blocks the transport's dispatch task.
- Four-way parity: if this repo's Java/Python/TypeScript siblings exist, observable behavior should
  match — same config shape, same metric names.
- Builders/facades are the construction path (`messaging()`, `MessageBuilder`, `MetricBuilder`) —
  never hand-built topics or envelopes.
- Runtime artifacts (vaults, parameter caches, generated streams, TLS certs, logs, build output,
  local broker state) stay out of Git.
