# <<COMPONENTNAME>> — Documentation

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` (`<<COMPONENTFULLNAME>>`) is a sink component: it consumes messages off the bus
and delivers each one to a destination, idempotently and with verified, retried, reported delivery. A
sink is the last thing standing between data and its destination — it consumes work, delivers it
outward, and only then lets go of the source.

```text
  consume ──► deliver (idempotent, stable key) ──► verify ──► confirm ──► report
                       ▲                                                    │
                       └────────── retry with full jitter ◄─────────────────┘
```

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold's local destination end to end |
| **[How-to guides](how-to-guides.md)** | accomplish a specific task — add a destination, tune retry, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, or metric |
| **[Explanation](explanation.md)** | understand the archetype — why the ordering exists, and why each step earns its place |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What does this config option do?"** → [Reference — Configuration](reference/configuration.md).
- **"What message do I send / receive on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"How do I add a real destination?"** → [How-to — Add a destination](how-to-guides.md#add-a-destination).

## Audience

These docs describe the archetype as generated — delivery to the local-filesystem reference
destination — for whoever picks up this repo next. Once you add a real destination, rewrite these
pages to describe it instead.
