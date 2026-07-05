# Design — Core runtime model (platform × transport)

> Companion to [README.md](README.md) and [REQUIREMENTS.md](REQUIREMENTS.md). **Status: Phase 1
> SHIPPED on `main` (v0.2.0), all four languages — Phase 2+ still proposed.**
> This is the heart of the change: the two-axis runtime model, platform profiles, the precedence
> resolver, auto-detection, the new CLI contract, identity resolution, the preserved init order, the
> schema additions, and the **full Phase-0 behavior-preserving migration** — all shipped. Java is
> canonical; per-language deltas are in [PARITY.md](PARITY.md). `file:line` citations are to the current tree.

---

## 1. Today: one overloaded axis

`ParsedCommandLine.Mode ∈ {GREENGRASS, STANDALONE}` (`ParsedCommandLine.java:16`) is parsed from
`-m/--mode` (`GGCommons.java:384-405`) and consumed at exactly one runtime branch — the transport
switch in the messaging client constructor (`MessagingClient.java:42-61`):

```
GREENGRASS  -> GreengrassMessagingProvider     (Nucleus IPC)
STANDALONE  -> StandaloneMessagingProvider(MessagingConfiguration.loadFromFile(path), thingName)   (dual-MQTT)
```

That single enum silently encodes **three** separable concerns:

1. **transport** — IPC vs dual-MQTT (the only thing the runtime actually branches on);
2. **default config provider** — there is *no* mode→provider mapping today; the default is the
   library-wide `GG_CONFIG` whenever `-c` is omitted (`GGCommons.java:380`), even under STANDALONE;
3. **platform** — "on a Nucleus" vs "everywhere else"; **purely implicit**, no code represents it.

IPC is **hard-locked to the Nucleus**: the IPC provider talks the Greengrass IPC protocol over the
Nucleus-provided domain socket and Nucleus-injected env (`AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT`,
`SVCUID`); the GG_CONFIG/SHADOW/CONFIG_COMPONENT config sources also require IPC
(`ConfigProviderBuilder.java:37-57`). So `transport=IPC ⇒ platform=GREENGRASS` is a real runtime
dependency, not a stylistic choice.

## 2. Target: two orthogonal axes + a profile

```
                 ┌─────────────── platform (primary) ───────────────┐
                 │  GREENGRASS        HOST            KUBERNETES      │   ← what host facilities exist;
                 └───────────────────────────────────────────────────┘     picks per-subsystem DEFAULTS
                 ┌──────────────── transport (derived) ──────────────┐
                 │  IPC               MQTT            MQTT            │   ← how messaging works;
                 └───────────────────────────────────────────────────┘     overridable, validated
```

- **`--platform {GREENGRASS,HOST,KUBERNETES,auto}`** (default `auto`) — the primary selector. A
  platform is a named **profile**: a table of default providers/targets/sinks for each subsystem
  (§3).
- **`--transport {IPC,MQTT}`** — secondary, defaults from the platform (GREENGRASS→IPC, HOST→MQTT,
  KUBERNETES→MQTT), independently overridable, but **constrained** (IPC only valid on GREENGRASS).

The axes are *constrained-orthogonal*: profiles for the **other seven** subsystems vary by platform
independently of transport; only messaging-transport is platform-coupled, and only via the IPC lock.

## 3. Platform profiles (the default tables)

A profile is pure data. The resolver consults it only for *unset* settings (§4). Connectivity model:
**edge-first with intermittent cloud cooperation** — the KUBERNETES profile assumes cloud services are
*used* but the link is intermittent, so its defaults are disconnect-tolerant (offline-first, store-and-
forward, in-cluster broker for local continuity).

| Setting | `GREENGRASS` | `HOST` | `KUBERNETES` |
|---|---|---|---|
| transport (default) | `IPC` | `MQTT` | `MQTT` |
| config source | `GG_CONFIG` | `FILE` | `CONFIGMAP` |
| metrics target | `log` (current default; direct `cloudwatch` preferred for cloud push — `cloudwatchcomponent` completeness-only) | `log` | `prometheus` |
| logging sink | file (Nucleus log dir) | console + file | **stdout-JSON**, no rotation |
| heartbeat surface | messaging topic | messaging/metric | messaging/metric **+ HTTP health endpoint** |
| credentials KeyProvider | `kms` (SDK chain → TES) | `file` | **`env`/`file` (offline-capable default)**; `kms` via IRSA only with an offline fallback (FR-CRED-6) |
| credentials central sync | `awsSecretsManager` (SDK chain → TES) | `none` | `awsSecretsManager` (SDK chain) or ESO/CSI mount |
| parameters source | `awsSsm` (TES) | `mountedDir`/`env` | `mountedDir` (ConfigMap/Secret) or `awsSsm` (chain) |
| streaming buffer | Nucleus work dir | local path | **PVC mount path** (StatefulSet) |
| identity (Thing) | `AWS_IOT_THING_NAME` | `-t`/`AWS_IOT_THING_NAME` | Downward-API (pod/ns/annotation) |
| AWS auth | SDK chain (resolves TES on-device) | SDK default chain | SDK default chain (IRSA / IAM-Roles-Anywhere) |

Every cell below "transport" and "config source" is a **default only** — explicit flags/config win
(§4). The GREENGRASS and HOST columns are *exactly today's behavior*, re-expressed as data: that is
what makes Phase 0 behavior-preserving (§7).

## 4. The precedence resolver

A single `resolveProfile()` per library produces a resolved settings object consumed by every
subsystem initializer. One rule governs every defaultable setting:

```
resolve(setting) =
    explicit CLI flag           if present          # e.g. -c FILE x.json, --transport MQTT
 ▸  explicit config value       if present & legal  # only from inputs available pre-config (see §6)
 ▸  platform-profile default    from the resolved platform
 ▸  library default             the hard-coded fallback (e.g. log target, GG_CONFIG)
```

Pseudocode (language-agnostic; mirrors the four-way contract):

```text
function resolveProfile(flags, env):
    platform = flags.platform == AUTO ? detectPlatform(env) : flags.platform     # §5
    profile  = PROFILES[platform]                                                # §3 table
    transport = flags.transport ?? profile.transport
    validate(platform, transport)            # §4.1 — reject IPC × {HOST,KUBERNETES}
    configSource = flags.configSource ?? profile.configSource                    # default-provider injection
    identity = resolveIdentity(flags.thing, platform, env)                       # §6.2
    log("platform=%s (basis=%s) transport=%s configSource=%s", platform, basis, transport, configSource)
    return ResolvedProfile{ platform, transport, configSource, identity, profile }
```

### 4.1 Invalid-combination guard (FR-RT-5)

`validate(platform, transport)` rejects, at startup, with a precise error:
- `transport==IPC && platform != GREENGRASS` → *"IPC transport requires --platform GREENGRASS (the
  Nucleus provides the IPC socket); got platform=…"*.
- (Rust only) `platform==GREENGRASS && transport==IPC` but the binary was built without the
  `greengrass` cargo feature → fail fast instead of today's silent `Ok(None)` messaging
  (`lib.rs:499-502`). The resolver reconciles compile-time capability with runtime platform.

### 4.2 Where the resolver slots in (from the code map)

`resolveProfile()` runs **right after arg parse, before messaging init**, where only parse-time inputs
exist:

| Lang | Insert between | Transport injection | Default-config-provider injection |
|---|---|---|---|
| Java | `GGCommons.java:112` (`processArgs`) → `:116` (messaging) | `MessagingClient.java:42` (switch on resolved transport, not `cmdLine.mode`) | `GGCommons.java:378-381` (replace hardcoded `GG_CONFIG`) |
| Rust | `lib.rs:252` → `:261` (`init_messaging`) | `lib.rs:461-463` (take `&Transport`) | `cli.rs:158-161` |
| TS | `ggcommons.ts:197` → `:202` | `ggcommons.ts:330-333` (`initMessaging`) | `cli.ts:72` |
| Python | `_process_args` (`ggcommons.py:73`) → `_init_messaging` (`:78`) | `MessagingClient.init` | `ggcommons.py:139` |

Note (FR-RT-6 / R5): the default-provider value is computed *inside* the pure `parseArgs` today, so the
resolved platform must be threaded into the default step — either parse, then re-default with the
platform, or pass the platform into the parser. This is the one structural wrinkle and it touches the
CLI module in all four langs (the `schema-and-scaffold` inventory enumerates the exact sites).

## 5. Auto-detection (`--platform auto`, the default)

Detection uses strong, well-defined signals in this order (first match wins); always overridable by an
explicit `--platform`; the decision and its basis are always logged (FR-RT-4):

1. **GREENGRASS** — `AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT` set, or `SVCUID` set, or the
   Nucleus IPC socket path present. (These are Nucleus-injected and exist nowhere else — definitive.)
2. **KUBERNETES** — the projected service-account token at
   `/var/run/secrets/kubernetes.io/serviceaccount/token` exists (primary, definitive);
   `KUBERNETES_SERVICE_HOST` is a secondary/confirming signal only.
3. **HOST** — fallback.

The GREENGRASS-before-KUBERNETES ordering is **load-bearing, not mutually exclusive**: a containerized
Nucleus component can set both Greengrass and Kubernetes signals, and GREENGRASS must win.

Today `AWS_IOT_THING_NAME` is already probed for thing-name (`ConfigManagerFactory.java:84-92`); the
detector reuses the same kind of env probe. Detection picks **defaults only**; it never overrides an
explicit flag or explicit config. (This is the design answer to the "magic" concern raised in review:
auto-detect is a default-*picker*, pinnable via `--platform`, never a behavior-*forcer*.)

## 6. CLI contract & identity

### 6.1 New surface (replaces `-m`)

```
--platform  GREENGRASS | HOST | KUBERNETES | auto      (default auto)
--transport IPC | MQTT                                  (default: from platform; validated)
-c/--config <SOURCE> [args]                             (default: from platform profile)
-t/--thing  <name>                                      (default: from platform identity)
```

- `-m`/`--mode` is **removed**; passing it errors with guidance to `--platform`/`--transport`
  (FR-RT-1, NFR-COMPAT-2). No alias shim (accepted breaking change).
- The awkward positional `-m STANDALONE <messaging_config.json>` disappears: MQTT broker/TLS config is
  read from the active config source (§ DESIGN-subsystems messaging), e.g. a ConfigMap + Secret on k8s
  or a file on a host.
- Builders gain equivalent `platform(...)`/`transport(...)` setters (FR-RT-8).

### 6.2 Identity resolution (FR-RT-7)

```
thing = -t/--thing                                                   if present
      ▸ platform identity:
          GREENGRASS  -> AWS_IOT_THING_NAME (Nucleus)
          KUBERNETES  -> Downward-API: annotation `ggcommons.io/thing-name`,
                         else `${namespace}.${pod-name}` (or `${namespace}.${node}`)
          HOST        -> AWS_IOT_THING_NAME if set
      ▸ library fallback  ("NOT_GREENGRASS" today — `ConfigManagerFactory.java:91`, Rust mirror `lib.rs:254-256`)
```

The resolved value passes the existing template sanitizer (`ConfigManager.sanitize`,
`ConfigManager.java:364`) so `{ThingName}`/`{ComponentName}` substitution is unchanged. Downward-API
**labels/annotations** are exposable only via a downwardAPI **volume** (whose items use `fieldRef` for
`metadata.*`), **not** via `env.valueFrom.fieldRef`; conversely `spec.nodeName`/`status.podIP` are
available **only** as env `fieldRef`, never in the volume. The chart therefore mounts a downwardAPI volume
for the `ggcommons.io/thing-name` annotation + namespace/pod-name and uses env `fieldRef` for node name
(see DESIGN-packaging §3).

## 7. Init order: preserved, with one inserted step

The canonical subsystem init order (Java `GGCommons.init` `:109-149`, mirrored in all four) is
**unchanged** except for the resolver step inserted before messaging:

```
processArgs
  → resolveProfile()                ← NEW (parse-time inputs only; §4.2)
  → messaging      (transport from resolved profile)
  → config         (default source from resolved profile; messaging passed for IPC-backed sources)
  → metrics → heartbeat
  → credentials → parameters → streaming        (credentials before streaming: $secret refs)
  → configManager.completeInitialization()       (last; listeners fire after)
```

**The messaging-before-config circularity is respected (FR-RT-6).** Transport is chosen before
component config loads, so an explicit `transport`/`platform` *config* override cannot come from the
component config section. Allowed override inputs, in order of preference: (a) CLI flags; (b) env; (c)
the messaging-config payload, which *is* read before the provider connects (`MessagingClient.java:50`).
A `transport`/`platform` key inside the main component config document is therefore **advisory/validation
only**, not authoritative for selection — documented explicitly to avoid a chicken-and-egg trap.

Partial-init cleanup (Python tears down on failed init, `ggcommons.py:108-117`) must extend to the new
resolver step: a resolver/validate failure must not leak an already-built messaging client.

## 8. Schema additions (additive; semantics preserved)

The canonical `schema/ggcommons-config-schema.json` top level is strict
(`additionalProperties:false, required:["component"]`, `:325-326`). New sections are **added** to
`properties{}` + `definitions{}` (never renames); the only enum *edit* is appending `"prometheus"`.
Every change is a **6-file commit** (canonical + 5 synced copies) gated by `schema/sync-schema.sh
--check` in CI (FR-SCHEMA-2).

| New | Shape | Purpose |
|---|---|---|
| `properties.platform` | `{ kind: greengrass\|host\|kubernetes, … }` | declares/validates the platform; lets platform-dependent heartbeat measures degrade cleanly |
| `properties.transport` | tagged-union on `type` (`ipc\|dualMqtt\|iotCoreOnly\|localBrokerOnly`); `$ref` to `definitions.mqttBroker` | selects transport; references (does not replace) the existing `messaging` section |
| `properties.health` | `definitions.healthProbes`: `liveness/readiness/startup` `{path,port,intervalSecs}` + enable toggle | the HTTP probe endpoint surface (new) |
| `metricEmission.target` | append `"prometheus"` to the enum (`:87`); add a `prometheus` branch (`port`,`path`) to `targetConfig` (which is `additionalProperties:false`, `:128`) | the pull metrics target |
| `properties.identity` | `definitions.identity`: `{ provider: irsa\|iamRolesAnywhere\|tes\|static, roleArn, region, audience, tokenFile, trustAnchorArn, … }` | a shared AWS-auth declaration; per-subsystem `region`/`endpointUrl` remain as overrides |

**Canonical default ports** (single source of truth, referenced by the Helm chart, DESIGN-packaging
§4/§12): health endpoint `8081`; prometheus `/metrics` `9090`. Both are configurable (`health.port`,
`metricEmission.targetConfig.prometheus.port`) and default their listener bind to the pod IP (NFR-SEC-4).

`transport` vs `messaging` ownership is made explicit (R-schema): **`transport` selects, `messaging`
configures.** Existing STANDALONE-shaped `messaging` configs keep validating through the transition.

## 9. Phase 0 — behavior-preserving refactor (full detail)

**Goal:** introduce the entire two-axis machinery while changing **no externally observable runtime
behavior**. Success criterion: the existing test suites pass (re-pointed to the new CLI) and on-device
GREENGRASS behavior is byte-for-byte identical in its defaults. Phase 0 ships *before* any Kubernetes
feature.

**P0.1 — Introduce the abstractions (no wiring yet).** Add `Platform` (enum `GREENGRASS|HOST|KUBERNETES`),
`Transport` (enum `IPC|MQTT`), a `PlatformProfile` data type, the `PROFILES` table (only `GREENGRASS`
and `HOST` populated in Phase 0), and a pure `resolveProfile(flags, env)`including the §4.1 validation
and §5 detector. Unit-test the resolver and detector in isolation. *No call sites changed yet.*

**P0.2 — Re-express today's two modes as profiles.** Populate `PROFILES[GREENGRASS]` and
`PROFILES[HOST]` so that:
- `GREENGRASS.transport = IPC`, `GREENGRASS.configSource = GG_CONFIG` (matches the current library
  default), all AWS settings = TES-as-today.
- `HOST.transport = MQTT`, `HOST.configSource = GG_CONFIG` **(deliberately unchanged)** — today STANDALONE
  does *not* flip the default to FILE (verified: the default is `GG_CONFIG` even under STANDALONE,
  `GGCommons.java:380`), so to preserve behavior the HOST profile
  must keep `GG_CONFIG` as the default config source in Phase 0. (Changing it to FILE is a *behavior
  change*, so it is out of Phase 0; **decided for Phase 1** as a labeled change — see §10 / §12 #1.)

  > This is the subtle correctness point of Phase 0: profiles must reproduce *current* defaults, even
  > the surprising ones, or the suites move. Improvements to defaults are deferred to a later, labeled
  > change.

**P0.3 — Map the old CLI onto the new axes (internally).** Parse `--platform`/`--transport` as the new
public surface; map the legacy single `-m` token *during P0 only at the test-rewrite boundary*:
`-m GREENGRASS` ≡ `--platform GREENGRASS` (transport derives to IPC); `-m STANDALONE <path>` ≡
`--platform HOST --transport MQTT` with the path fed to the messaging-config loader. Since back-compat
is **not** a goal, the legacy token is then **removed** and the error path (NFR-COMPAT-2) added — but
doing the equivalence mapping first lets the existing suites be rewritten mechanically and proves the
new flags reproduce the old selections.

**P0.4 — Repoint the consumers (the three injection sites, §4.2).**
- Transport: replace `switch(cmdLine.mode)` in the messaging constructor/init with a switch on
  `resolved.transport` (`MessagingClient.java:42`; Rust `init_messaging` takes `&Transport`; TS
  `initMessaging`; Python `MessagingClient.init`).
- Default config provider: replace the hardcoded `GG_CONFIG` default with `resolved.configSource`
  (`GGCommons.java:378-381`; `cli.rs:158-161`; `cli.ts:72`; `ggcommons.py:139`).
- Identity: route the existing thing-name probe through `resolveIdentity` (no behavior change for
  GREENGRASS/HOST — same env probe).
- Insert `resolveProfile()` at the documented point before messaging init in each lib.

**P0.5 — Enforce the IPC lock (§4.1).** Add the validation so an impossible explicit combination fails
loudly instead of opaquely. For GREENGRASS/HOST defaults this is a no-op (the combinations they produce
are already valid), so behavior is unchanged.

**P0.5b — Parity fixes that ride along (decided §12).** Converge `receiveOwnMessages` to the
Java-canonical `true` in all four languages (TS `false`→`true`) and fix the Rust `false`-is-a-no-op so
the flag is honored. This is an accepted pre-1.0 behavior change for TS; it lands in Phase 0 alongside the
CLI rewrite so the suites move once. (The HOST `GG_CONFIG`→`FILE` default flip is **not** here — it is a
Phase-1 labeled change, §10 / §12 #1.)

**P0.6 — Rewrite tests + scaffolding to the new CLI.** The `schema-and-scaffold` inventory lists every
artifact that types the old contract: the four CLI parsers and their mode tests
(`cli.rs:218-268`, `cli.test.ts`, Python mode tests, Java mode parsing), the example `-m STANDALONE`
invocations (`examples/java/README.md:106`, etc.), the interop node entrypoints
(`test-infra/interop/*_node/*`), and the recipe `Run:` lines. These migrate **in the same change** so
the interop matrix and suites stay green. CI workflows themselves type no `-m` (only `python -m pytest`)
— the impact is via the tests and the schema-drift gate.

**P0.7 — Gate.** The Phase-0 PR is accepted only when: all pre-existing suites pass; interop 32/32;
vault vectors pass; `sync-schema.sh --check` green; and a manual GREENGRASS on-device smoke test shows
identical behavior. JaCoCo's 90% gate (Java) must still pass. *No Kubernetes code is in Phase 0.*

**Phase-0 invariant restated:** Phase 0 changes *how a deployment is launched and how defaults are
computed internally*, not *what any existing deployment does*. The platform table for GREENGRASS/HOST
is a faithful encoding of current behavior; Kubernetes and any default *improvements* are Phase 1+.

## 10. Phase 1 — the KUBERNETES profile (additive)

Populate `PROFILES[KUBERNETES]` (§3) and ship the native facilities (each detailed in
[DESIGN-subsystems.md](DESIGN-subsystems.md)): `CONFIGMAP` config source with directory-watch reload;
`prometheus` metrics target; stdout-JSON logging sink; the HTTP health endpoint + SIGTERM graceful
shutdown; Downward-API identity; PVC-aware streaming + StatefulSet guidance; ConfigMap/Secret-sourced
MQTT config (dual-MQTT default for edge-with-cloud-cooperation); the `env` KeyProvider; `--platform
auto` detection enabled by default; the Helm chart ([DESIGN-packaging.md](DESIGN-packaging.md)).
Cross-cutting: **disconnect tolerance** (NFR-DISCONNECT-1) verified by fault-injection across the
cloud-dependent subsystems. Phase 1 also lands the decided **HOST default config-source flip
`GG_CONFIG`→`FILE`** (§12 #1) — a labeled behavior change, announced and distinct from the Kubernetes
facilities above.

## 11. Phase 2+ — additive platforms

New deployment substrates (ECS, Nomad, systemd, bare-metal fleets) become additional `PROFILES`
entries + detectors, with no change to the resolver, the CLI contract, or the subsystem seams. This
is the payoff of naming the platform axis: growth is additive data, not new branches × four languages.

## 12. Decisions (resolved 2026-06-25)

These were the four open questions; all are now decided (recorded here; the affected sections above are
updated to match).

1. **HOST default config source → `FILE`.** The §3 target stands: HOST defaults to `FILE`, not
   `GG_CONFIG`. `GG_CONFIG` requires the Nucleus IPC that HOST lacks, so today's default is a latent
   footgun. **Phasing:** Phase 0 keeps `GG_CONFIG` (P0.2) so its behavior-preserving oracle stays clean;
   the flip to `FILE` lands as a **labeled behavior change in Phase 1** (§10).
2. **`receiveOwnMessages` → converge to `true` (Java-canonical), in Phase 0.** All four libraries default
   `true`; TS changes `false`→`true`; the Rust `false`-is-a-no-op is fixed so the flag is honored.
   Resolved now rather than carried as parity debt (the breaking change is already accepted). Lands in
   Phase 0 (P0.5b).
3. **Advisory `transport`/`platform` config key → validation-only.** A `platform`/`transport` key inside
   the component config is a **sanity-check only**: selection uses parse-time inputs (flags ▸ env ▸
   messaging-config payload, §7), and a mismatch between that key and the resolved value is a startup
   error. No two-pass init (avoids the R5 init-order circularity).
4. **Rust compile-time vs runtime platform → fail-fast.** When `platform=GREENGRASS` on a binary built
   without the `greengrass` cargo feature, the resolver **errors at startup** naming the missing feature
   (§4.1), instead of today's silent `Ok(None)` no-op messaging.
