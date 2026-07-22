# DESIGN — <<COMPONENTNAME>>

> Treat this document as the **design-fidelity contract** for this component: before changing
> behavior, update the relevant section here in the same change, and review new work against what
> is written here — not against a summary of it.

## What it is

`<<COMPONENTFULLNAME>>` is a general-purpose EdgeCommons component. This scaffold ships a
demonstrated monitoring/command surface (a periodic metric, data signal, event, and a custom command
verb) as a worked example; fill in the sections below as you replace it with real business logic.

## Decisions

Record each real design decision here as you make it, numbered so later sessions can cite it —
mirror the `D-<PREFIX>-<n>` convention used across the EdgeCommons repos (e.g. `D-<<COMPONENTNAME>>-1`).

- **D-<<COMPONENTNAME>>-1.** *(example — replace)* What this component actually does, and why it
  does not fit the `protocol-adapter`/`sink`/`processor` archetypes.

## Config

`config.schema.json` is the source of truth for `component.global`/`component.instances[]`; describe
here *why* each new key exists as you add it, not just its shape (the schema's `description` fields
cover shape).

## Command surface

`set-greeting` ships as the worked example of a custom verb registered before the inbox goes active.
Record here every verb you add, its shape, and its error codes.

## Metrics

`loopTicks` ships as the worked example. Record here every metric you add, its dimensions, and what
it is *for*.

## Validation

- `cargo test` — the custom command handler and app construction, no broker required.
- `cargo llvm-cov --fail-under-lines 90` — the coverage gate.
- Record here any additional validation this component needs and where it runs.
