# <<COMPONENTNAME>> — Documentation

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTFULLNAME>>` is a sink component: the last thing standing between data on the bus and its
destination. It consumes messages, delivers each one outward, verifies what landed, and only then
lets go of the source. Built on the `edgecommons` Rust library, it runs wherever you deploy it — a
Greengrass v2 component, a standalone HOST process, or a Kubernetes pod. It ships with a **local
filesystem destination** so it runs with no external backend; replace it with S3, HTTP, or whatever
you are delivering to.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — build the scaffold, run it, and watch a delivery land |
| **[How-to guides](how-to-guides.md)** | accomplish a task — plug in a real destination, tune retry, deploy, wire CI |
| **[Reference](reference/)** | look up an exact config key, topic, or metric |
| **[Explanation](explanation.md)** | understand the shape — the destination seam, verify-before-release, retry classification |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why verify before releasing the source?"** → [Explanation](explanation.md).
- **"Show me a complete config."** → [Sample Configurations](sample-configurations.md).

## Audience

These docs describe the component **as generated** — the local-filesystem destination, the exact
retry/event/metric behavior the scaffold registers. They are for whoever runs
`edgecommons component new` next and needs to know what they got before they start writing a real
destination backend.
