# DESIGN — <<COMPONENTNAME>>

> Treat this file as the design-fidelity contract for this component: the binding record of what it
> is supposed to do and why, that an agreed implementation must match. Keep it current in the same
> change as any behavior it describes — a stale design doc is a defect, not cosmetic drift.

## What it is

<!-- One paragraph: what does this adapter connect to, and why does it exist? Replace this line. -->
`<<COMPONENTNAME>>` is a southbound protocol adapter generated from the EdgeCommons
`java-protocol-adapter` template. It currently talks to an in-process simulated device
(`Device.SimBackend`); describe the real protocol here once `Device.java` is implemented against it.

## Decisions

<!-- Number your decisions D-<<COMPONENTNAME>>-1, D-<<COMPONENTNAME>>-2, ... as you make them, the
     same way the core library's DESIGN docs carry a decision register. Record the decision, the
     alternatives considered, and the consequence — not just the conclusion. -->

- D-<<COMPONENTNAME>>-1: _(none yet — this is a generated scaffold)_

## Config

<!-- Summarize component.global / component.instances[] here as you extend config.schema.json;
     the schema file and docs/reference/configuration.md are the detailed sources of truth. -->

See `config.schema.json` and `docs/reference/configuration.md`.

## Command surface

<!-- List the sb/* verbs this adapter actually serves, and any you add beyond the generic family
     registered in Commands.java. -->

See `docs/reference/messaging-interface.md`.

## Metrics

<!-- List southbound_health plus every operational metric family this adapter emits, including any
     protocol-specific families (Inventory/Poll/Publish) you add beyond the two worked examples in
     Metrics.java. -->

See `docs/reference/metrics.md`.

## Validation

<!-- Record what you actually validated this against: a real device/simulator, which platforms
     (HOST/Greengrass/Kubernetes), and what the live-sim integration test (LiveSimIT.java) covers. -->

- `mvn test` — unit suite against the sim backend.
- `LiveSimIT.java` — gated on `EC_LIVE_SIM`; run it against your real protocol once `Device.java` is
  implemented.
