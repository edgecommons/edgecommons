# DESIGN — <<COMPONENTNAME>>

> Treat this document as the **design-fidelity contract** for this component: before changing
> behavior, update the relevant section here in the same change, and review new work against what
> is written here — not against a summary of it.

## What it is

`<<COMPONENTFULLNAME>>` is a processing component: it subscribes to messages, transforms them
through a pipeline of stages, and forwards the result. This scaffold ships two worked stages
(`FieldEquals`, `CountPerTick`) as examples; fill in the sections below as you add your own.

## Decisions

Record each real design decision here as you make it, numbered so later sessions can cite it —
mirror the `D-<PREFIX>-<n>` convention used across the EdgeCommons repos (e.g. `D-<<COMPONENTNAME>>-1`).

- **D-<<COMPONENTNAME>>-1.** *(example — replace)* What routes this processor runs and why they are
  split the way they are (one route per concern, or one per source).
- **D-<<COMPONENTNAME>>-2.** *(example — replace)* Any stage whose semantics need explaining beyond
  its schema description (a windowing policy, a dedup key).

## Config

`config.schema.json` is the source of truth for `component.global`/`component.instances[]`/
`$defs.stage`; describe here *why* each new key or stage variant exists as you add it, not just its
shape (the schema's `description` fields cover shape).

## Command surface

This scaffold registers no custom command verbs beyond the library's automatic
`ping`/`reload-config`/`get-configuration`. Record here any verb you add (a "flush now" for a
windowed stage, a per-route pause) and its shape.

## Metrics

`processorThroughput` (`received`/`published`/`dropped`/`errors`) ships unchanged. Record here any
metric you add per stage or per route, its dimensions, and what it is *for*.

## Validation

- `cargo test` — pipeline mechanics + route config/dispatch, no broker required.
- `cargo llvm-cov --fail-under-lines 90` — the coverage gate.
- Record here any additional validation this component's stages need (a golden-file transform test,
  a load test for a windowed stage) and where it runs.
