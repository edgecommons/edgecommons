# DESIGN — <<COMPONENTNAME>>

This is the scaffold's design-fidelity contract: as you replace the demo surface and build out real
behavior, treat this document the same way the rest of the ecosystem does — a signed-off record of
what the component does and why, kept in sync with the code in the same change that lands it. A
decision recorded here and later changed should be updated, not silently left stale.

## What it is

_A general-purpose component doing `<<COMPONENTNAME>>`'s job. Replace this paragraph with a real
description of what the component does, its inputs/outputs, and the deployment context once known._

## Decisions

_Number decisions as you make them (`D-<<COMPONENTNAME>>-1`, `D-<<COMPONENTNAME>>-2`, …) so later
sessions can cite them. Record the decision, the alternatives considered, and why — not just the
outcome._

## Config

_Document `component.global`/`component.instances[]` additions as you make them — keep
`config.schema.json` and `docs/reference/configuration.md` as the source of truth and link to them
here rather than duplicating the option table._

## Command surface

_This scaffold ships one demo verb (`set-greeting`) beyond the library's built-ins (`ping`,
`reload-config`, `get-configuration`) — see `docs/reference/messaging-interface.md`. Record here
any verb you add, and its request/reply shape._

## Metrics

_This scaffold ships one demo metric, `loopTicks` (`tickCount`, `uptimeSecs`) — see
`docs/reference/metrics.md`. Record any metric you add here, including its dimensions and why each
is low-cardinality._

## Validation

_Record which validation paths apply (HOST/EMQX smoke, Greengrass lab deploy, Kubernetes) and their
current status — done, in progress, or not yet run — as you complete them. See the org umbrella's
validation matrix for the available infrastructure._
