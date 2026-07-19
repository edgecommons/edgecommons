# <<COMPONENTNAME>> — Documentation

*This documents the generated scaffold; rewrite it as you build the component out.*

`<<COMPONENTFULLNAME>>` is a Python **sink component** built on the `edgecommons` library: the last
thing standing between data and its destination. It consumes work off the bus, delivers it outward,
verifies what landed, and only then releases the source. It runs as a Greengrass v2 component, a
standalone HOST process, or a Kubernetes pod.

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold end to end and watch it deliver |
| **[How-to guides](how-to-guides.md)** | accomplish a task — write a destination, add a sink, deploy |
| **[Reference](reference/)** | look up an exact config option, topic, or metric |
| **[Explanation](explanation.md)** | understand the sink archetype and its non-negotiable ordering |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why verify before releasing the source? Why a time budget, not an attempt count?"** → [Explanation](explanation.md).

## Audience

These docs are for whoever picks up this scaffold next — the integrator wiring a real destination,
and the operator deploying it. They describe the component **as generated**; once you add a backend
to `app/dest.py` or a sink to config, update the pages that describe them (see `AGENTS.md`).
