# DESIGN — <<COMPONENTNAME>>

> Treat this file as the design-fidelity contract for this component: the binding record of what it
> is supposed to do and why, that an agreed implementation must match. Keep it current in the same
> change as any behavior it describes — a stale design doc is a defect, not cosmetic drift.

## What it is

<!-- One paragraph: what business problem does this component solve? Replace this line. -->
`<<COMPONENTNAME>>` is a generated EdgeCommons component scaffold, currently running its demo
metric/signal/event/command-verb quartet. Describe the real component here once you replace it.

## Decisions

<!-- Number your decisions D-<<COMPONENTNAME>>-1, D-<<COMPONENTNAME>>-2, ... as you make them, the
     same way the core library's DESIGN docs carry a decision register. Record the decision, the
     alternatives considered, and the consequence — not just the conclusion. -->

- D-<<COMPONENTNAME>>-1: _(none yet — this is a generated scaffold)_

## Config

<!-- Summarize component config here as you extend config.schema.json; the schema file and
     docs/reference/configuration.md are the detailed sources of truth. -->

See `config.schema.json` and `docs/reference/configuration.md`.

## Command surface

<!-- List every custom command verb this component registers beyond the library built-ins. -->

See `docs/reference/messaging-interface.md`.

## Metrics

<!-- List every metric this component emits. -->

See `docs/reference/metrics.md`.

## Validation

<!-- Record what you actually validated this against: which platforms (HOST/Greengrass/Kubernetes),
     and any integration/system tests beyond the unit suite. -->

- `mvn test` — unit suite.
