---
name: Propose a new component
about: Propose a new adapter, processor, or sink for the ecosystem
title: "New component: "
labels: new-component
---

**Component**

- Proposed name (flat, lowercase): <!-- e.g. bacnet-adapter -->
- Category: adapter / processor / sink
- Language: JAVA / PYTHON / RUST / TYPESCRIPT
- Protocol / target (if any): <!-- e.g. BACnet, Kafka -->

**What it does**

<!-- One paragraph: the data it moves and how. -->

**Platforms**

<!-- GREENGRASS / HOST / KUBERNETES -->

**Notes**

<!-- Existing prototype? Dependencies? Anything the maintainers should know. -->

> On acceptance: scaffold with `ggcommons create-component`, push to `edgecommons/<name>`, wire the
> reusable CI, set topics, and open a PR adding it to `edgecommons/registry`.
