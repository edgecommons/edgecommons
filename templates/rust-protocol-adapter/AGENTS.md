# <<COMPONENTNAME>> — component notes

EdgeCommons **southbound protocol adapter** (Rust). Full name `<<COMPONENTFULLNAME>>`, crate/binary
`<<BINNAME>>`. Depends on the `edgecommons` Rust library. If this repo lives inside the EdgeCommons
org umbrella workspace, read its root `AGENTS.md` first (org repo map, design-fidelity contract,
validation matrix, platform/transport model); everything below is this component's own detail.

## What it is

Connects to devices, reads signals, and publishes them onto the Unified Namespace (UNS) in the
shape the rest of the fleet expects: `SouthboundSignalUpdate` on the `data` class, the canonical
`southbound_health` metric plus two worked operational families, and the generic `sb/*` command
family (SOUTHBOUND.md §2.2 equivalent) on the command inbox. Ships with a simulated device backend
(`src/device.rs`'s `SimBackend`) so it runs with no hardware. Runs on `GREENGRASS` / `HOST` /
`KUBERNETES` via `edgecommons` — no platform branching in this component's own code.

## The seam

`src/device.rs`'s `DeviceSession`/`DeviceBackend` trait pair is the one place protocol knowledge
lives. Everything above it (`src/app.rs`'s connect/poll/backoff supervisor, `src/commands.rs`'s
`sb/*` verbs, `src/metrics.rs`'s families) is written against the trait and does not change when a
new protocol is added. **The boundary rule:** a backend knows protocols; it does not know
EdgeCommons topics, the UNS, envelopes, or metrics.

## Config location

This component's own settings live under `component.global` / `component.instances[]` in the
EdgeCommons config document (`config.schema.json` is the contract); the sibling sections (`tags`,
`hierarchy`, `identity`, `messaging`, `metricEmission`, `logging`, `heartbeat`) are the standard
`edgecommons` envelope, owned by the canonical schema and not redeclared here. `test-configs/`
carries a runnable example.

## Validation expectations

- `cargo test` covers every module against the simulator and a mocked device-control channel — no
  network, no broker, no device required.
- `cargo llvm-cov --fail-under-lines 90` is the coverage gate (`.github/workflows/ci.yml`'s
  `coverage` job) — the org rule is 90% line coverage per language; live-infra-only paths (a real
  protocol talking to real hardware) are validated through lab/HOST smoke instead of being forced
  into unit coverage. Do not lower the gate or exclude testable code to pass it.
- `tests/live_sim.rs` is a **self-skipping** live suite, gated on `EC_LIVE_SIM` — it must show as
  skipped in a normal `cargo test` and pass when pointed at a real simulator/device.
- `edgecommons component validate` checks this repo's config against `config.schema.json` and warns
  if `Cargo.lock` is not committed.

## Org conventions this scaffold inherits

- Southbound contract: a data point is a **signal**, never a "tag" (EdgeCommons envelope `tags` is
  unrelated business metadata).
- Writes are allow-listed by stable `signal.id`, checked before any device I/O; the default is
  read-only.
- Four-way parity: if this repo's Java/Python/TypeScript siblings exist, observable command/metric
  behavior should match — same verbs, same error codes, same measure names.
- Builders/facades are the construction path (`data()`, `events()`, `commands()`, `MetricBuilder`) —
  never hand-built topics or envelopes.
- Runtime artifacts (vaults, parameter caches, generated streams, TLS certs, logs, build output,
  local broker state) stay out of Git.
