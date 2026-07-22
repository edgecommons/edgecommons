# <<COMPONENTNAME>> — Documentation

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTFULLNAME>>` is a general-purpose EdgeCommons component built on the `edgecommons` Rust
library. It gives you the library's standard CLI contract, configuration, logging, messaging,
metrics, and heartbeat out of the box, and demonstrates the rest of the monitoring/command surface
an edge-console reads — a periodic metric, a periodic data signal, a periodic event, and a custom
command verb — so a freshly generated component has something live to observe and command from
day one. Runs wherever you deploy it — a Greengrass v2 component, a standalone HOST process, or a
Kubernetes pod.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — build the scaffold, run it, and watch the demo surface |
| **[How-to guides](how-to-guides.md)** | accomplish a task — add your own metric/signal/event/command, deploy, wire CI |
| **[Reference](reference/)** | look up an exact config key, topic, or metric |
| **[Explanation](explanation.md)** | understand the shape — facades, instance connectivity, config hot-reload |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why does the code use facades instead of raw publish?"** → [Explanation](explanation.md).
- **"Show me a complete config."** → [Sample Configurations](sample-configurations.md).

## Audience

These docs describe the component **as generated** — the demo metric/signal/event/command quartet
`src/app.rs` ships. They are for whoever runs `edgecommons component new` next and needs to know
what they got before they replace the demo with real business logic.
