This documents the generated scaffold; rewrite it as you build the component out.

# <<COMPONENTNAME>> — Documentation

`<<COMPONENTFULLNAME>>` is a general-purpose EdgeCommons component: the standard CLI contract,
configuration, logging, messaging, metrics, and heartbeat, plus a small demonstrated monitoring and
command surface (a metric, a data signal, an event, and a custom command verb) so a freshly
generated component has something to show on an edge-console before you write any business logic.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold and watch its demo surface on the UNS |
| **[How-to guides](how-to-guides.md)** | accomplish a task — add a metric/signal/event/verb, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, or payload |
| **[Explanation](explanation.md)** | understand the shape — facades, identity, instance connectivity |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why facades instead of hand-built topics?"** → [Explanation](explanation.md).

## Audience

These docs are for **integrators and operators** — people who deploy this component and write
clients that consume or command it. They describe the scaffold exactly as generated; once you
replace the demo surface in `src/app.ts` with real business logic, rewrite them to describe that
instead.
