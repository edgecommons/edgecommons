# DESIGN — <<COMPONENTNAME>>

This is the scaffold's design-fidelity contract: as you replace the local-filesystem destination
and build out real behavior, treat this document the same way the rest of the ecosystem does — a
signed-off record of what the component does and why, kept in sync with the code in the same change
that lands it. A decision recorded here and later changed should be updated, not silently left
stale.

## What it is

_A sink component that delivers work to `<<COMPONENTNAME>>`'s destination. Replace this paragraph
with a real description of the destination, the durability/consistency guarantees it offers, and
the deployment context once known._

## Decisions

_Number decisions as you make them (`D-<<COMPONENTNAME>>-1`, `D-<<COMPONENTNAME>>-2`, …) so later
sessions can cite them. Record the decision, the alternatives considered, and why — not just the
outcome. Seed entries:_

- `D-<<COMPONENTNAME>>-1` — Destination choice: which backend implements `src/dest.ts`'s
  `Destination`, and how it satisfies the idempotent-key and verify-before-release properties.
- `D-<<COMPONENTNAME>>-2` — Retry policy defaults: why the shipped `baseDelayMs`/`maxDelayMs`/
  `giveUpAfterMs` are set the way they are for this destination's failure characteristics.

## Config

_Document `component.global`/`component.instances[]` additions as you make them — keep
`config.schema.json` and `docs/reference/configuration.md` as the source of truth and link to them
here rather than duplicating the option table._

## Command surface

_This scaffold registers no custom commands beyond the library's built-ins (`ping`,
`reload-config`, `get-configuration`). Record here any verb you add, and why a sink needed one
(e.g. a manual retry-now, a pause)._

## Metrics

_This scaffold ships one component-wide metric, `sinkDeliveries` (`received`/`delivered`/`retried`/
`exhausted`/`dropped`) — see `docs/reference/metrics.md`. Record any per-sink or per-destination
metric you add here, including its dimensions and why each is low-cardinality._

## Validation

_Record which validation paths apply (HOST/EMQX smoke, Greengrass lab deploy, Kubernetes, and any
live destination integration test) and their current status — done, in progress, or not yet run —
as you complete them. See the org umbrella's validation matrix for the available infrastructure._
