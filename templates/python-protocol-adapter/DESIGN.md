# DESIGN — <<COMPONENTNAME>>

> Treat this document as the design-fidelity contract for this component: before changing behavior,
> update the relevant section here in the same change: what changed, why, and what it means for
> config/commands/metrics/validation. A build that compiles but drifts from what this document says
> is not done.

## What it is

<<COMPONENTNAME>> is a `<<COMPONENTFULLNAME>>` southbound protocol adapter built on the
`edgecommons` Python library, implementing `docs/SOUTHBOUND.md`. Describe here, once you implement a
real protocol: which device family it targets, what its `signal_id` scheme is, and how it maps onto
the generic `sb/*` surface.

## Decisions

Record each significant design decision as it's made, numbered so later sessions can cite it:

- **D-1.** _(example)_ — replace with your first real decision (e.g. the stable `signal_id` scheme
  for your protocol, or why `browse()` is/isn't implemented).

## Config

What `component.global` and each `component.instances[]` device mean for this adapter — in
particular, what keys live under `connection` for your protocol (it is deliberately open in
`config.schema.json`). Keep this section's claims verified against `config.schema.json` — if they
disagree, the schema is the source of truth and this section is stale.

## Command surface

The generic `sb/*` family ships by default (`sb/status`, `sb/read`, `sb/write`, `sb/signals`,
`sb/browse`, `sb/pause`, `sb/resume`, `reconnect`, `repoll`). Document any protocol-specific behavior
here — e.g. what `browse()` actually enumerates, or any additional verb you add.

## Metrics

`southbound_health` (the exact SOUTHBOUND.md §5 set) plus `<<COMPONENTNAME>>Connection`/
`<<COMPONENTNAME>>Command` ship by default. Document any `Inventory`/`Poll`/`Publish` family you add:
name, dimensions (keep them low-cardinality), measures, and what each one means operationally.

## Validation

What "this component works" means in practice for this repo: which tests must pass
(`python -m pytest`, no broker/device needed — `test_live_sim.py` self-skips), the coverage gate (90%
line coverage — see `.github/workflows/ci.yml`), the live-sim integration test
(`EC_LIVE_SIM=<endpoint> pytest tests/test_live_sim.py`) against a real simulator/device, and any
platform-specific smoke test (HOST/Greengrass/Kubernetes) this component needs before a change ships.
