# DESIGN — <<COMPONENTNAME>>

This is the scaffold's design-fidelity contract: as you replace the simulator and build out real
behavior, treat this document the same way the rest of the ecosystem does — a signed-off record of
what the component does and why, kept in sync with the code in the same change that lands it. A
decision recorded here and later changed should be updated, not silently left stale.

## What it is

_A protocol adapter connecting to `<<COMPONENTNAME>>`'s devices and publishing their signals onto
the UNS. Replace this paragraph with a real description of the protocol, the device population, and
the deployment context once known._

## Decisions

_Number decisions as you make them (`D-<<COMPONENTNAME>>-1`, `D-<<COMPONENTNAME>>-2`, …) so later
sessions can cite them. Record the decision, the alternatives considered, and why — not just the
outcome. Seed entries:_

- `D-<<COMPONENTNAME>>-1` — Backend choice: which protocol/library implements `src/device.ts`'s
  `DeviceBackend`, and why.
- `D-<<COMPONENTNAME>>-2` — Write policy: which signals are configured writable by default, and the
  operational rationale for the allow-list contents.

## Config

_Document `component.global`/`component.instances[]` additions as you make them — keep
`config.schema.json` and `docs/reference/configuration.md` as the source of truth and link to them
here rather than duplicating the option table._

## Command surface

_This scaffold ships the full generic `sb/*` family (`status`/`read`/`write`/`signals`/`browse`/
`pause`/`resume`) plus `reconnect`/`repoll` — see `docs/reference/messaging-interface.md`. Record
here any verb you add beyond that set, and why the generic family wasn't enough._

## Metrics

_This scaffold ships the canonical `southbound_health` plus two worked operational families
(`<<COMPONENTNAME>>Connection`, `<<COMPONENTNAME>>Command`) — see `docs/reference/metrics.md`.
Record any protocol-specific family you add (inventory/poll/publish) here, including its dimensions
and why each is low-cardinality.

## Validation

_Record which validation paths apply (HOST/EMQX smoke, a live-sim gate against a real simulator,
Greengrass lab deploy, Kubernetes) and their current status — done, in progress, or not yet run —
as you complete them. See the org umbrella's validation matrix for the available infrastructure._
