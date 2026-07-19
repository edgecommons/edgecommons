# DESIGN — <<COMPONENTNAME>>

> Treat this file as the design-fidelity contract for this component: the binding record of what it
> is supposed to do and why, that an agreed implementation must match. Keep it current in the same
> change as any behavior it describes — a stale design doc is a defect, not cosmetic drift.

## What it is

<!-- One paragraph: what does this processor transform, and why? Replace this line. -->
`<<COMPONENTNAME>>` is a processing component generated from the EdgeCommons `java-processor`
template. It currently runs the two demo stages (`fieldEquals`, `countPerTick`) on one route;
describe the real pipeline here once you replace them.

## Decisions

<!-- Number your decisions D-<<COMPONENTNAME>>-1, D-<<COMPONENTNAME>>-2, ... as you make them, the
     same way the core library's DESIGN docs carry a decision register. Record the decision, the
     alternatives considered, and the consequence — not just the conclusion. -->

- D-<<COMPONENTNAME>>-1: _(none yet — this is a generated scaffold)_

## Config

<!-- Summarize the routes/pipeline config here as you extend config.schema.json; the schema file
     and docs/reference/configuration.md are the detailed sources of truth. -->

See `config.schema.json` and `docs/reference/configuration.md`.

## Command surface

<!-- This archetype has no custom command verbs by default — list any you add. -->

See `docs/reference/messaging-interface.md`.

## Metrics

<!-- List processorThroughput plus any per-route or per-stage metrics you add. -->

See `docs/reference/metrics.md`.

## Validation

<!-- Record what you actually validated this against: which platforms (HOST/Greengrass/Kubernetes),
     and any load/throughput testing beyond the unit suite. -->

- `mvn test` — the archetype's guard rails: 0..N stages, `onTick`, the self-echo guard.
