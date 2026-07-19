# <<COMPONENTNAME>> — Documentation

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` (`<<COMPONENTFULLNAME>>`) is a processing component: it subscribes to messages,
transforms them through a pipeline of stages, and forwards the result. The library gives you config,
messaging, metrics, and lifecycle; this template gives you the *processor archetype* — subscribe,
transform, forward — so you write only the transformation.

```text
  subscribe(filter) ──► bounded queue ──► one worker thread per route ──► publish
                                             (Pipeline)                  local | northbound
```

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold's demo route end to end |
| **[How-to guides](how-to-guides.md)** | accomplish a specific task — add a stage, tune a route, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, or metric |
| **[Explanation](explanation.md)** | understand the archetype — routes, the 0..N stage contract, the two guards |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What does this config option do?"** → [Reference — Configuration](reference/configuration.md).
- **"What message do I send / receive on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"How do I add my own transformation?"** → [How-to — Add a stage](how-to-guides.md#add-a-stage).

## Audience

These docs describe the archetype as generated — the two demo stages (`fieldEquals`, `countPerTick`)
running on one route — for whoever picks up this repo next. Once you add your own stages, rewrite
these pages to describe your actual pipeline instead.
