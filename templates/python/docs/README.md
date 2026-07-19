# <<COMPONENTNAME>> — Documentation

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTFULLNAME>>` is a Python component built on the `edgecommons` library. It gives you
configuration, messaging, metrics, logging, and the heartbeat keepalive for free, and demonstrates
the rest of the monitoring + command surface an edge-console reads — a periodic metric, a periodic
data signal, a periodic event, and a custom command verb — in [`app/<<COMPONENTNAME>>.py`](../app/<<COMPONENTNAME>>.py).
It runs as a Greengrass v2 component, a standalone HOST process, or a Kubernetes pod.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold end to end and watch it on the bus |
| **[How-to guides](how-to-guides.md)** | accomplish a task — replace the demo state, add a real metric/signal/event, deploy |
| **[Reference](reference/)** | look up an exact config option, topic, or metric |
| **[Explanation](explanation.md)** | understand the shape of a *service* component and why it looks this way |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why does the scaffold look like this?"** → [Explanation](explanation.md).

## Audience

These docs are for whoever picks up this scaffold next — the integrator or operator deploying it,
and the developer replacing the demo code with real business logic. They describe the component
**as generated**; once you change `app/<<COMPONENTNAME>>.py` or `config.schema.json`, update the
pages that describe them (see `AGENTS.md`).
