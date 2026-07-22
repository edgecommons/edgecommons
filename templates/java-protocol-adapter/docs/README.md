# <<COMPONENTNAME>> — Documentation

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` (`<<COMPONENTFULLNAME>>`) is a southbound protocol adapter: it connects to
devices, publishes their signals onto a message bus as `SouthboundSignalUpdate`, and serves an
on-demand read/write/browse command surface. It runs against an in-process simulated device out of
the box, so you can see the whole shape before you connect a real protocol.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold against the bundled sim, end to end |
| **[How-to guides](how-to-guides.md)** | accomplish a specific task — connect a real protocol, deploy, observe health |
| **[Reference](reference/)** | look up an exact config key, topic, payload, or data-type mapping |
| **[Explanation](explanation.md)** | understand the archetype — the seam, the worker, the two planes |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What does this config option do?"** → [Reference — Configuration](reference/configuration.md).
- **"What message do I send / receive on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"How are values represented on the wire?"** → [Reference — Data Types](reference/data-types.md).
- **"How do I replace the simulator with my protocol?"** → [How-to — Implement a real device backend](how-to-guides.md#implement-a-real-device-backend).
- **"Is it connected and healthy?"** → [How-to — Observe health and status](how-to-guides.md#observe-health-and-status).

## Audience

These docs describe the archetype as generated — the sim-backed adapter, its command surface, and
its metrics — for whoever picks up this repo next: you in three months, a teammate, or an agent. They
are not end-user product docs; once `Device.java` talks to a real protocol, rewrite these pages to
describe that protocol instead of the simulator.
