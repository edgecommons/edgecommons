# DESIGN — <<COMPONENTNAME>>

> Treat this document as the **design-fidelity contract** for this component: before changing
> behavior, update the relevant section here in the same change, and review new work against what
> is written here — not against a summary of it.

## What it is

`<<COMPONENTFULLNAME>>` is a sink component: it consumes messages, delivers each one to a
destination, verifies what landed, and reports every transition. This scaffold ships a local
filesystem destination as a worked example; fill in the sections below as you add a real backend.

## Decisions

Record each real design decision here as you make it, numbered so later sessions can cite it —
mirror the `D-<PREFIX>-<n>` convention used across the EdgeCommons repos (e.g. `D-<<COMPONENTNAME>>-1`).

- **D-<<COMPONENTNAME>>-1.** *(example — replace)* Which destination(s) this sink delivers to, and
  why (a real object store, an HTTP endpoint, a database).
- **D-<<COMPONENTNAME>>-2.** *(example — replace)* What counts as a transient vs. permanent failure
  for your backend (drives the retry-vs-give-up choice in `deliver_with_retry`).

## Config

`config.schema.json` is the source of truth for `component.global`/`component.instances[]`/
`$defs.destination`; describe here *why* each new destination variant or key exists as you add it,
not just its shape (the schema's `description` fields cover shape).

## Command surface

This scaffold registers no custom command verbs beyond the library's automatic
`ping`/`reload-config`/`get-configuration`. Record here any verb you add (a "retry now", a
per-destination pause) and its shape.

## Metrics

`sinkDeliveries` (`received`/`delivered`/`retried`/`exhausted`/`dropped`) ships unchanged. Record
here any metric you add per destination (bytes transferred, per-object latency), its dimensions, and
what it is *for*.

## Validation

- `cargo test` — the destination contract + sink config/retry/connectivity, no real backend
  required.
- `cargo llvm-cov --fail-under-lines 90` — the coverage gate.
- Record here any additional validation your destination needs (a local emulator, a lab bucket) and
  where it runs.
