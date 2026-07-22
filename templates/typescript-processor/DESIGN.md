# DESIGN — <<COMPONENTNAME>>

This is the scaffold's design-fidelity contract: as you replace the demo stages and build out real
behavior, treat this document the same way the rest of the ecosystem does — a signed-off record of
what the component does and why, kept in sync with the code in the same change that lands it. A
decision recorded here and later changed should be updated, not silently left stale.

## What it is

_A processing component that subscribes, transforms, and forwards. Replace this paragraph with a
real description of the routes this component owns, what they filter/aggregate/project, and why,
once known._

## Decisions

_Number decisions as you make them (`D-<<COMPONENTNAME>>-1`, `D-<<COMPONENTNAME>>-2`, …) so later
sessions can cite them. Record the decision, the alternatives considered, and why — not just the
outcome. Seed entries:_

- `D-<<COMPONENTNAME>>-1` — Stage inventory: which stages this component ships beyond the two demo
  ones, and what each does.
- `D-<<COMPONENTNAME>>-2` — Route topology: how many routes, what each subscribes to, and why they
  are split the way they are (rather than one route with a longer pipeline).

## Config

_Document `component.global`/`component.instances[]` additions as you make them — keep
`config.schema.json` and `docs/reference/configuration.md` as the source of truth and link to them
here rather than duplicating the option table._

## Command surface

_This scaffold registers no custom commands beyond the library's built-ins (`ping`,
`reload-config`, `get-configuration`) — a processor typically has nothing to command beyond its
config. Record here any verb you add, and why a processor needed one._

## Metrics

_This scaffold ships one component-wide metric, `processorThroughput` (`received`/`published`/
`dropped`/`errors`) — see `docs/reference/metrics.md`. Record any per-route or per-stage metric you
add here, including its dimensions and why each is low-cardinality._

## Validation

_Record which validation paths apply (HOST/EMQX smoke, Greengrass lab deploy, Kubernetes) and their
current status — done, in progress, or not yet run — as you complete them. See the org umbrella's
validation matrix for the available infrastructure._
