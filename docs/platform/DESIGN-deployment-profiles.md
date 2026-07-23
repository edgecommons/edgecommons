# DESIGN — one topology, per-platform profiles

> **Status: PROPOSED (2026-07-23).** A change to the deployment-definition language so a single
> **plant topology** is deployed to any platform, replacing today's N divergent per-platform
> definitions. Companion to `DESIGN-cli.md` (§8); adds a decision to that register when accepted.

## 1. Problem

A `DeploymentDefinition` targets exactly one platform (`targetStandard.family`) and **inlines
everything** — the config source, the artifact shape, and whole blocks that only exist for one
platform:

- HOST: `localBroker`, `auxiliaries`, `configProvider`, per-component `launch` (supervisord).
- Greengrass: per-component `artifact.{version,digest,greengrassName}`, `configSource: GG_CONFIG`.
- Kubernetes: a container `image` (not expressible at all today).

So the Dallas plant exists as **N divergent definitions**: `dallas/` (HOST, the full plant) and
`dallas-gg/` (Greengrass, a **3-component subset** — filling line only, no modbus/file-replicator —
that also carries its **own duplicated copy** of the config layers). They have already drifted.

**North star: define the plant once; deploy it to any platform.** One Dallas topology proves HOST,
Greengrass, and Kubernetes, from one source of truth.

## 2. Model

Split the definition into a shared **topology** and per-platform **profiles**.

### 2.1 `topology` — the plant, platform-agnostic

- `hierarchy`: levels + scopes (each with its layer file) — unchanged.
- `nodes[]`: `{ key, scope, identity, components[] }`, where a component is
  `{ name, catalogKey, layer, messaging }` — its functional identity and the `layers/` it merges.
  **No** `configSource`, **no** `artifact`, **no** `launch`.

The topology says *what runs where* and *each component's functional config*. Nothing about *how it
is delivered*.

### 2.2 `profiles` — how the topology is delivered on a platform

A map keyed by a profile name. Each profile:

| Field | Meaning |
|---|---|
| `family` | `HOST` \| `GREENGRASS` \| `KUBERNETES`. |
| `environments[]` | Bindings + protection, per platform (HOST `local`/open, GG `prod`/protected, K8s `local`). |
| `deploys` | Optional selection over the topology: which nodes/components this profile deploys. **Default and expectation: the complete plant** — every profile deploys the whole topology unless a component genuinely cannot exist on that platform. `deploys` is the escape hatch for that rare case, not the norm. |
| `defaults` | Per-profile delivery defaults (e.g. `configSource`, artifact source kind). |
| `nodes` | Keyed by topology node key: per-node platform adornments + a per-component delivery overlay. |

Per-component delivery overlay, by family:

- **HOST** — node: `localBroker`, `auxiliaries`, `configProvider`; component: `{ artifact:{source}, configSource, launch }`.
- **GREENGRASS** — component: `{ artifact:{version,digest,greengrassName}, configSource: GG_CONFIG }`.
- **KUBERNETES** — component: `{ image, configSource, resources?, probes? }` (the rest defaults to the k8s test chart's proven values).

### 2.3 Effective definition (what a renderer sees)

For a selected profile P, and for each node/component P `deploys`, the **effective component** =
`topology component ⊕ profiles[P] overlay` (deep-merge, profile wins — the same `layered.rs`
semantics the config lineage already uses). The result is the **same flat shape the renderers consume
today**, so the renderers change only in *how they are fed*, not in *what they emit*.
`deployment render --profile <name>` selects the profile; `--target <family>` continues to check the
family matches.

## 3. Concrete — Dallas `gw-fill-01 / opcua-adapter` in all three profiles

```yaml
# ---- topology (shared) --------------------------------------------------------
topology:
  nodes:
    - key: gw-fill-01
      scope: line/filling-line
      identity: { thingName: gw-fill-01 }
      components:
        - name: opcua-adapter
          catalogKey: OpcUaAdapter
          layer: layers/components/filling-line/opcua-adapter.json
          messaging: { clientId: fill-opcua, type: mqtt, file: opcua-messaging.json }

# ---- profiles (per-platform delivery) -----------------------------------------
profiles:
  host:
    family: HOST
    environments: [{ name: local, protection: open, bindings: bindings/local.json }]
    defaults: { configSource: CONFIG_COMPONENT }
    nodes:
      gw-fill-01:
        localBroker: { kind: emqx, port: 1883, ... }
        auxiliaries: [{ name: field-sim, command: /opt/simenv/bin/python /opt/sims/dallas_filling_sim.py, ... }]
        configProvider: { configSource: FILE, artifact: { source: { kind: sibling, repo: config-component } }, ... }
        components:
          opcua-adapter:
            artifact: { source: { kind: sibling, repo: opcua-adapter } }
            launch: { order: 30, waitFor: ["localhost:1883","localhost:4840"], exec: "java ... -jar /app/opcua/app.jar", ... }

  greengrass:
    family: GREENGRASS
    environments: [{ name: prod, protection: protected, bindings: bindings/prod.json }]
    defaults: { configSource: GG_CONFIG }
    # No `deploys` — Greengrass deploys the COMPLETE plant (every node, every component).
    nodes:
      gw-fill-01:
        components:
          opcua-adapter:
            artifact: { version: "1.0.0", digest: "sha256:9f1c22aa5d3e", greengrassName: com.mbreissi.edgecommons.OpcUaAdapter }

  kubernetes:
    family: KUBERNETES
    environments: [{ name: local, bindings: bindings/k8s.json }]
    defaults: { configSource: CONFIG_COMPONENT }
    nodes:
      gw-fill-01:
        components:
          opcua-adapter:
            image: "ghcr.io/edgecommons/opcua-adapter:1.0.0"
```

The `opcua-adapter`'s **functional** config (`layers/components/filling-line/opcua-adapter.json` — the
OPC UA connection, subscriptions, deadbands) is written **once**, in the topology. Only *delivery*
differs per profile.

## 4. Merge & validation

- Deep-merge `topology-component ⊕ profile-component` (objects merge; scalars/arrays replace) — the
  `layered.rs` rule already used for config lineage.
- **Validation:** every component a profile `deploys` must resolve to a legal per-platform shape —
  HOST needs `launch` + `artifact.source`; GG needs `version`+`digest`+`greengrassName`; K8s needs
  `image`. Missing → a new `EC50xx`. A `deploys` entry naming an unknown node/component → error.
- The existing semantic rules (S-1..S-9) run on the **effective per-profile** definition, unchanged.

## 5. Migration — the three Dallas fixtures become one

- `dallas/` → `topology` + `profiles.host` (the full plant: `dallas-console`, `gw-fill-01`,
  `gw-pack-01`).
- `dallas-gg/` → `profiles.greengrass`, **expanded to the complete plant** (every node, every
  component — no longer the filling-line subset); its **duplicated layers deleted** (it shares the
  topology's). Its GG golden is regenerated for the complete plant (§7 step 4).
- New `profiles.kubernetes` — the complete plant, per-component images.
- **One** `layers/` set, shared by every profile.

**Canonical plant (working assumption, correct me):** the topology is the full HOST plant —
`dallas-console` (edge-console + config provider + broker), `gw-fill-01` (opcua, modbus, telemetry,
file-replicator, uns-bridge), `gw-pack-01` (opcua, modbus, telemetry, uns-bridge). The GG fixture's
former extra `gw-fill-02` (a horizontal-scaling demo of two per-thing deployments on one line) is
folded in as a topology node so every platform can render it, or dropped — decided in step 4.

**No backward compatibility.** `topology` + `profiles` is the *only* definition shape; the flat
`targetStandard` + top-level `nodes` form is removed. Every fixture and test workspace migrates
(including the minimal `write_minimal_workspace` used by the CLI integration tests).

## 6. Renderer & golden impact

- `workspace.rs` gains an "effective workspace for profile P" step (topology ⊕ profile) feeding the
  existing renderers.
- HOST and Greengrass renderers consume the effective workspace and **must stay byte-identical to
  today's goldens** — the migration is a refactor, not a behavior change, and the existing golden
  tests (`dallas_golden`, `dallas_gg_golden`) are the proof.
- New Kubernetes renderer + K8s golden.

## 7. Staging (each step independently verifiable)

1. **Schema + model + merge** — `topology`/`profiles`, the effective-workspace step, validation. No
   renderer change.
2. **Feed HOST + GG renderers from the effective workspace, refactor-only** — migrate `dallas/` and
   `dallas-gg/` to the unified shape *preserving their current output*, and prove `dallas_golden` +
   `dallas_gg_golden` are **unchanged byte-for-byte**. This isolates "the profiles plumbing is
   correct" from any intended behavior change. Delete the divergent copies + dallas-gg's duplicated
   layers here.
3. **Complete the Greengrass profile** — expand `profiles.greengrass` from the filling-line subset to
   the **whole plant**, and regenerate `dallas_gg_golden` as a *reviewed* diff (this golden is
   *meant* to change; step 2 already proved the plumbing is neutral).
4. **Kubernetes renderer** + `profiles.kubernetes` (complete plant) + a new K8s golden.
5. **bct's drift gate** renders the `host` profile of the unified definition.
6. **Docs + register** (new D-CLI decision) + PLAN.

## 8. Open questions

- **Profile key vs. `--target`.** Profiles keyed by a name, each declaring `family`; `--profile`
  selects, `--target` remains a family check. (No backward-compat path — the flat shape is gone.)
- **Greengrass completeness — the HOST-runtime scaffolding.** GG deploys the complete plant, but some
  HOST elements are runtime scaffolding, not deployable components: the per-node **EMQX `localBroker`**
  (GG has the nucleus's local broker), the **field-sim `auxiliaries`** (test simulators), and the
  **FILE-based `configProvider`** (GG uses `GG_CONFIG` natively). Working assumption for step 3: the
  GG profile deploys every real **component** (opcua, modbus, telemetry, file-replicator, uns-bridge,
  **edge-console**, and **config-component** as a component) on every node, and the broker/sim/FILE-
  provider scaffolding is HOST-profile-only. `edge-console` on Greengrass is the one to confirm when
  its golden is reviewed.
- **K8s node model.** A topology node (a gateway) becomes a set of Deployments; named-node placement
  (REVIEW #11) maps the node key to a `nodeSelector`/label. Settled in step 4, against the k8s golden.
