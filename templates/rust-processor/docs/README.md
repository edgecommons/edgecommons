# <<COMPONENTNAME>> — Documentation

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTFULLNAME>>` is a processing component: it subscribes to messages already on the bus,
transforms them through an ordered pipeline of stages, and forwards the result — to the local bus
or northbound. Built on the `edgecommons` Rust library, it runs wherever you deploy it — a
Greengrass v2 component, a standalone HOST process, or a Kubernetes pod. It ships with two worked
stages (a filter and a rollup) so it runs and produces output with no external dependency.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — build the scaffold, run it, and watch a rollup cross the bus |
| **[How-to guides](how-to-guides.md)** | accomplish a task — write a new stage, add a route, deploy, wire CI |
| **[Reference](reference/)** | look up an exact config key, topic, or metric |
| **[Explanation](explanation.md)** | understand the shape — the pipeline, self-echo, the bounded queue |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why messaging() and not data()?"** → [Explanation](explanation.md).
- **"Show me a complete config."** → [Sample Configurations](sample-configurations.md).

## Audience

These docs describe the component **as generated** — the two shipped stages, the exact routes and
metrics the scaffold registers. They are for whoever runs `edgecommons component new` next and
needs to know what they got before they start writing their own pipeline stages.
