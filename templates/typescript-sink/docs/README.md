This documents the generated scaffold; rewrite it as you build the component out.

# <<COMPONENTNAME>> — Documentation

`<<COMPONENTFULLNAME>>` is the last thing standing between data and its destination: it consumes
work off the bus, delivers it outward, and only lets go of the source once delivery is *verified*.
It ships with a **local-filesystem destination** (write-temp-then-rename, no network) so it runs
end to end with nothing to configure; replace the destination in `src/dest.ts` with your real
backend and everything above the seam — retry, verification, reporting — keeps working unchanged.

| Doc | Start here when you want to… |
|-----|------------------------------|
| **[Tutorial](tutorial.md)** | learn by doing — run the scaffold and watch a delivery land |
| **[How-to guides](how-to-guides.md)** | accomplish a task — implement a destination, tune retry, add a sink, deploy |
| **[Reference](reference/)** | look up an exact config key, topic, payload, or metric |
| **[Explanation](explanation.md)** | understand the archetype — deliver, verify, retry, report |

## Quick routing

- **"I'm new here."** → [Tutorial](tutorial.md).
- **"What config option does X?"** → [Reference — Configuration](reference/configuration.md).
- **"What message on which topic?"** → [Reference — Messaging Interface](reference/messaging-interface.md).
- **"What does this metric mean?"** → [Reference — Metrics](reference/metrics.md).
- **"Why verify before confirming?"** → [Explanation](explanation.md).

## Audience

These docs are for **integrators and operators** — people who deploy this component and need to
know what happens to the data it's handed. They describe the scaffold exactly as generated; once
you implement a real destination in `src/dest.ts`, rewrite them to describe your backend instead of
the local filesystem.
