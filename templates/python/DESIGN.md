# DESIGN — <<COMPONENTNAME>>

> Treat this document as the design-fidelity contract for this component: before changing behavior,
> update the relevant section here in the same change: what changed, why, and what it means for
> config/commands/metrics/validation. A build that compiles but drifts from what this document says
> is not done.

## What it is

<<COMPONENTNAME>> is a `<<COMPONENTFULLNAME>>` component built on the `edgecommons` Python library.
Describe here, in your own words once you build it out: what problem it solves, what it consumes and
produces, and which platforms (GREENGRASS / HOST / KUBERNETES) it targets.

## Decisions

Record each significant design decision as it's made, numbered so later sessions can cite it:

- **D-1.** _(example)_ — replace with your first real decision.

## Config

What `component.global` and `component.instances[]` mean for this component, and why each key exists.
Keep this section's claims verified against `config.schema.json` — if they disagree, the schema is
the source of truth and this section is stale.

## Command surface

Every custom command verb this component registers beyond the library built-ins
(`ping`/`reload-config`/`get-configuration`/`status`): what it does, its request/reply shape, and its
error codes.

## Metrics

Every metric this component defines beyond the library's own (heartbeat `sys` measures): name,
dimensions (keep them low-cardinality), measures, and what each one means operationally.

## Validation

What "this component works" means in practice for this repo: which tests must pass
(`python -m pytest`, no broker/device needed), the coverage gate (90% line coverage — see
`.github/workflows/ci.yml`), and any platform-specific smoke test (HOST/Greengrass/Kubernetes) this
component needs before a change ships.
