This documents the generated scaffold; rewrite it as you build the component out.

# <<COMPONENTNAME>> — Documentation

`<<COMPONENTFULLNAME>>` connects to devices, polls their signals, and republishes them onto the
Unified Namespace (UNS) as `SouthboundSignalUpdate` messages — the same normalized shape every
southbound adapter in the fleet uses, regardless of protocol. It ships with an in-process **simulated
backend** (two signals, no hardware, no network) so it runs immediately; replace the backend in
`src/device.ts` with your protocol and everything above the seam — polling, backoff, health,
metrics, the `sb/*` command surface — keeps working unchanged.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold against its simulator and watch it on the UNS |
| **[How-to guides](how-to-guides.md)** | accomplish a task — read/write signals, add a device, replace the simulator, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, payload, metric, or type |
| **[Explanation](explanation.md)** | understand the adapter archetype — the seam, the control channel, quality |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"How does a reading become a value?"** → [Reference — Data Types](reference/data-types.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why is the seam shaped this way?"** → [Explanation](explanation.md).

## Audience

These docs are for **integrators and operators** — people who deploy the component and write
clients that consume or command it. They describe the scaffold exactly as generated; once you
implement a real protocol in `src/device.ts`, rewrite them to describe your device instead of the
simulator.
