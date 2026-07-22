# <<COMPONENTNAME>> — Documentation

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTFULLNAME>>` connects to devices, reads signals, and publishes them onto the Unified
Namespace (UNS) in the shape the rest of the fleet expects: it polls a source and republishes value
changes as `SouthboundSignalUpdate` messages, and serves the standardized `sb/*` command surface plus
`southbound_health`. It is built on the `edgecommons` (`edgecommons`) library and runs as a
Greengrass v2 component, a standalone HOST process, or a Kubernetes pod.

Out of the box it runs against an **in-process simulator** (`adapter: sim`) — no PLC, no hardware —
so a fresh scaffold connects, publishes, and answers commands on the first run. Replace the simulator
with your protocol behind the same seam (`<<SNAKENAME>>/device.py`).

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — bring the adapter up against the simulator, end to end |
| **[How-to guides](how-to-guides.md)** | accomplish a task — implement your protocol, add a device, read/write, deploy |
| **[Reference](reference/)** | look up an exact config option, topic, payload, or type |
| **[Explanation](explanation.md)** | understand how the adapter archetype works and why |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"How is a reading turned into a published value?"** → [Reference — Data Types](reference/data-types.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why a seam? Why one worker per device?"** → [Explanation](explanation.md).

## Audience

These docs are for whoever picks up this scaffold next — the integrator implementing a real
protocol backend, and the operator deploying it. They describe the component **as generated** (the
simulator, the actual `sb/*` verbs and metrics the uplifted code registers); once you implement
`DeviceSession`/`DeviceBackend` for your protocol, update the pages that describe them (see
`AGENTS.md`).
