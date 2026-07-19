This documents the generated scaffold; rewrite it as you build the component out.

# <<COMPONENTNAME>> — Documentation

`<<COMPONENTFULLNAME>>` subscribes to messages on the bus, transforms them through a pipeline of
stages, and forwards the result. It ships with two demo stages (`fieldEquals`, a filter, and
`countPerTick`, a stateful rollup) wired into one route, so the pipeline is observable end to end
before you write a single stage of your own.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the shipped route and watch a rollup appear |
| **[How-to guides](how-to-guides.md)** | accomplish a task — write a stage, add a route, tune the queue, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, payload, or metric |
| **[Explanation](explanation.md)** | understand the archetype — the pipeline shape, the self-echo guard |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why does a processor need a self-echo guard?"** → [Explanation](explanation.md).

## Audience

These docs are for **integrators and operators** — people who deploy this component and write
clients that feed it or consume its output. They describe the scaffold exactly as generated; once
you implement real stages in `src/proc.ts`, rewrite them to describe your pipeline instead of the
demo one.
