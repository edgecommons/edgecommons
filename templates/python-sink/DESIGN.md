# DESIGN — <<COMPONENTNAME>>

> Treat this document as the design-fidelity contract for this component: before changing behavior,
> update the relevant section here in the same change: what changed, why, and what it means for
> config/commands/metrics/validation. A build that compiles but drifts from what this document says
> is not done.

## What it is

<<COMPONENTNAME>> is a `<<COMPONENTFULLNAME>>` sink component built on the `edgecommons` Python
library: it consumes messages off the bus and delivers them to a destination outside EdgeCommons.
Describe here, once you build it out: what it consumes, where it delivers, and what "delivered"
means for your destination (a file landing, a row committed, an HTTP 2xx).

## Decisions

Record each significant design decision as it's made, numbered so later sessions can cite it:

- **D-1.** _(example)_ — replace with your first real decision (e.g. why this sink's destination was
  chosen, or how its key scheme was derived).

## Config

What each sink (`component.instances[]` entry) means for this component: its subscription filter, its
destination, and its retry policy. Keep this section's claims verified against `config.schema.json`
— if they disagree, the schema is the source of truth and this section is stale.

## Command surface

This scaffold registers no custom command verbs beyond the library built-ins
(`ping`/`reload-config`/`get-configuration`/`status`). Document any you add here: request/reply shape
and error codes.

## Metrics

`sinkDeliveries` (`received`/`delivered`/`retried`/`exhausted`/`dropped`) ships by default. Document
any metric you add: name, dimensions (keep them low-cardinality), measures, and what each one means
operationally.

## Validation

What "this component works" means in practice for this repo: which tests must pass
(`python -m pytest`, no broker/device needed), the coverage gate (90% line coverage — see
`.github/workflows/ci.yml`), and any platform-specific smoke test (HOST/Greengrass/Kubernetes) this
component needs before a change ships.
