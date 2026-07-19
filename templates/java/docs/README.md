# <<COMPONENTNAME>> — Documentation

> This documents the generated scaffold; rewrite it as you build the component out.

`<<COMPONENTNAME>>` (`<<COMPONENTFULLNAME>>`) is a minimal EdgeCommons component: the library gives
you config, messaging, metrics, logging, heartbeat, and the command inbox; this scaffold demonstrates
the rest of the monitoring/command surface an edge-console reads — a metric, a data signal, an event,
and a custom command verb — so a freshly generated component has something live to look at instead of
an empty dashboard.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold and watch its demo surface end to end |
| **[How-to guides](how-to-guides.md)** | accomplish a specific task — add your own metric/signal/event/verb, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, or payload |
| **[Explanation](explanation.md)** | understand the shape — why identity is config-driven, what each facade owns |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What does this config option do?"** → [Reference — Configuration](reference/configuration.md).
- **"What message do I send / receive on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"How do I add my own business logic?"** → [How-to — Replace the demo surface](how-to-guides.md#replace-the-demo-surface-with-your-own-business-logic).

## Audience

These docs describe the archetype as generated — the demo metric/signal/event/verb quartet — for
whoever picks up this repo next. Once you replace the demo surface with real business logic, rewrite
these pages to describe that logic instead.
