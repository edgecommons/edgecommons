# Analysis — HTTP(S) as a component-to-component transport (non-Greengrass)

> **Status: ANALYSIS ONLY — nothing proposed here is accepted, and nothing is implemented.**
> Commissioned question: *many Kubernetes microservice deployments use HTTP(S) rather than an async
> broker for component-to-component communication, and HOST/standalone services could be deployed
> that way too. How feasible is adding that to EdgeCommons for non-Greengrass platforms?*
>
> Scope: **HOST** and **KUBERNETES** only. GREENGRASS is excluded by construction — `--transport IPC`
> is hard-locked to the Nucleus ([`DESIGN-core.md`](DESIGN-core.md) §1, §4.1) and the Nucleus supplies
> the socket, so there is no HTTP question there.
>
> Companion reading: [`DESIGN-core.md`](DESIGN-core.md) (the platform × transport axes),
> [`DESIGN-channels.md`](DESIGN-channels.md) (the three-channel model),
> [`UNS-CANONICAL-DESIGN.md`](UNS-CANONICAL-DESIGN.md) (topic grammar and reserved classes).

---

## 1. Verdict up front

**Feasible, but not as a drop-in replacement for MQTT, and the honest cost is much larger than
"write a fourth provider."**

The finding that drives everything else: **EdgeCommons traffic is not one shape, it is two**, and the
two have opposite relationships to HTTP.

| Half | What it is | HTTP fit |
|---|---|---|
| **Addressed** — `request`/`reply`, the command inbox, config push | The caller knows exactly who it is talking to. Correlation ids, deadlines, one responder. | **Native.** This *is* RPC. HTTP is arguably a better fit than MQTT here. |
| **Fan-out** — every `publish` of `state`/`metric`/`cfg`/`log`/`data`/`evt`/`app` | The publisher does not know, and must not know, who is listening. | **Not native.** HTTP is client→server; there is no "publish to whoever cares." |

The addressed half is roughly the inbound surface of a component. The fan-out half is the outbound
surface — and it is the half that *defines the product*, because the Unified Namespace exists
precisely so that publishers and consumers do not know about each other.

So the accurate framing is not "MQTT vs HTTP." It is:

> **You can move the addressed half to HTTP with contained, well-bounded work. Moving the fan-out
> half to HTTP means writing and operating a broker that speaks HTTP — because a rendezvous point is
> not an MQTT artifact, it is what pub/sub *is*.**

Anyone who reads `--transport HTTP` as "same behavior, different protocol, no broker to run" will be
wrong, and will discover it late. That is the single most important thing this document exists to say.

---

## 2. What the transport seam actually is

The seam is genuinely clean, and this is the good news.

`Transport` is a two-valued enum
([`platform/Transport.java:13-18`](../../libs/java/src/main/java/com/mbreissi/edgecommons/platform/Transport.java)),
resolved once at startup and consumed at **exactly one branch per language**
([`DESIGN-core.md`](DESIGN-core.md) §4.2 maps all four):

```java
// libs/java/src/main/java/com/mbreissi/edgecommons/messaging/MessagingClient.java:57-77
MessagingClient(ParsedCommandLine cmdLine, boolean receiveOwnMessages) {
    switch (cmdLine.transport) {
        case IPC:  this.messagingProvider = new GreengrassMessagingProvider(receiveOwnMessages); break;
        case MQTT: this.messagingProvider = new StandaloneMessagingProvider(config, cmdLine.thingName); break;
        default:   throw new RuntimeException("Invalid transport specified: " + cmdLine.transport);
    }
}
```

Everything above that line — heartbeat, metrics, logging, config, commands, the UNS facades — talks to
the abstract `MessagingProvider` and never learns which transport it got. Adding a third enum value is
a small, mechanical, already-mapped structural change.

The contract a third provider must satisfy
([`MessagingProvider.java:236-315`](../../libs/java/src/main/java/com/mbreissi/edgecommons/messaging/MessagingProvider.java)):

| Method family | Semantics required |
|---|---|
| `publish` / `publishRaw` | fire-and-forget to a topic |
| `publishConfirmed` | **positive** transport acknowledgement or throw (`:187-202`) |
| `subscribe` | topic **filter** (wildcards), per-subscription concurrency + queue bound |
| `subscribeAcknowledged` | subscription confirmed by the transport or throw (`:247-255`) |
| `unsubscribe` | tear down a filter |
| `request` / `reply` / `cancelRequest` | correlation id + ephemeral reply address + framework-owned deadline (`:299-308`) |
| `…Northbound` variants of all of the above | channel 2 — always MQTT by design ([`DESIGN-channels.md`](DESIGN-channels.md) §Current state) |
| `connected()` | **local-only** readiness signal; a northbound outage must not flip `/readyz` |

Note the last two rows. The northbound family is defined as MQTT and stays MQTT — an HTTP transport
is a **channel-1 (local bus) concern only**, which is consistent with the existing decision that
`--transport` is the channel-1 axis and nothing else ([`DESIGN-channels.md:62`](DESIGN-channels.md)).

---

## 3. Seven things that make this *more* feasible than expected

These are grounded in the current tree, and several are the direct result of earlier design choices
that happened to buy this optionality.

**3.1 No retained messages anywhere.** `RepublishListener` was built specifically so a site view
rehydrates **without broker retain**
([`uns/RepublishListener.java:39`](../../libs/java/src/main/java/com/mbreissi/edgecommons/uns/RepublishListener.java)).
An HTTP transport does not have to emulate MQTT retain — the largest single semantic gap simply is not
there. Even the one specialized last-will path hard-wires `retain = false`
([`messaging/provider/mqtt.rs:357-367`](../../libs/rust/src/messaging/provider/mqtt.rs)).

**3.2 No Last-Will in the generic contract.** `messaging.lwt` is *explicitly rejected* by config
validation — "uns-bridge derives its site Last-Will internally"
([`MessagingConfiguration.java:95-97`](../../libs/java/src/main/java/com/mbreissi/edgecommons/messaging/MessagingConfiguration.java)),
and `MqttLastWill` is deliberately kept out of the schema. So there is no "how does HTTP express a
will message" problem for ordinary components.

**3.3 Clean session — no durable subscriptions today.** The MQTT provider connects with
`set_clean_session(true)` ([`mqtt.rs:621`](../../libs/rust/src/messaging/provider/mqtt.rs)). A
subscriber that is down **already** misses messages. An HTTP transport with the same property is not a
regression, and no store-and-forward guarantee has to be reproduced.

**3.4 The envelope is already transport-agnostic protobuf.** `Message.to_bytes()`/`from_bytes()`
produce the same bytes regardless of transport. Over HTTP the body is those bytes with
`Content-Type: application/x-protobuf`. **Zero wire-contract change**, so `uns-test-vectors/` and the
envelope conformance suites carry over untouched.

**3.5 One injection site per language, already mapped.** [`DESIGN-core.md`](DESIGN-core.md) §4.2
tabulates the exact file:line for Java, Rust, TS, and Python. This is the rare structural change where
the map already exists.

**3.6 An embedded HTTP server already ships in all four languages.** `HealthServer` (Java
`com.sun.net.httpserver`; Rust a hand-rolled `TcpListener` at
[`health.rs:185`](../../libs/rust/src/health.rs); Python `ThreadingHTTPServer`; TS `node:http`) plus
the Prometheus `/metrics` endpoint. "The component listens on a port" is an established, shipped
pattern, and the k8s templates already expose `8081`/`9090` with all three probes wired.

**3.7 Topic-filter matching already exists in-library.** Every language carries a Paho-derived
`topicMatchesSub` helper
([`MessagingProvider.java:317`](../../libs/java/src/main/java/com/mbreissi/edgecommons/messaging/MessagingProvider.java)).
If matching has to move from the broker into our code, the primitive is written and tested.

---

## 4. Six things that make it harder than it looks

**4.1 Fan-out needs a rendezvous point, and there are only bad ways to avoid one.**

This is the crux. For a publisher to reach N unknown subscribers over HTTP, one of these must be true:

- *Publisher-side fan-out with a discovery registry.* The publisher learns its subscribers and POSTs
  N times. This **breaks the UNS decoupling outright** — the publisher now knows its consumers — and
  costs O(N·M) requests. Not acceptable against the product's core premise.
- *A hub that accepts POSTs and fans out.* Workable, and honest naming matters here: **this is a
  broker.** You would be writing an HTTP broker, owning its backpressure, delivery, subscription
  registry, wildcard matching, and HA story. You do not remove the broker; you replace a mature one
  (EMQX) with one you maintain.
- *Everything becomes addressed.* This is the actual idiomatic k8s microservice answer — service A
  calls service B by DNS name. But EdgeCommons traffic is largely *not* addressed, so this is a
  different product, not a different transport.

The mismatch is architectural, not protocol-level. **HTTP does not remove the broker from a pub/sub
system; it only changes who writes it.**

**4.2 `subscribe` has no HTTP analogue without a long-lived stream.** Server-Sent Events is the
closest fit — one-directional server→client, survives k8s Services and ingress, plain HTTP. But SSE is
text-framed, so protobuf bodies need base64 (~33% overhead) unless you drop to chunked binary or
WebSocket. And an SSE stream *is* a connection, which reintroduces the connection-lifecycle,
reconnect, and resubscribe machinery that MQTT already provides for free. The "HTTP is connectionless
and therefore simpler" intuition does not survive contact with `subscribe`.

**4.3 Rust has no HTTP stack, by deliberate choice.** [`libs/rust/Cargo.toml`](../../libs/rust/Cargo.toml)
has no `hyper`, `axum`, `reqwest`, or `tiny-http`. The health server is hand-rolled on
`std::net::TcpListener` *specifically to avoid the dependency*. A production HTTP transport
(routing, keep-alive, chunked encoding, concurrency, backpressure, TLS) is not hand-rollable at
acceptable quality. This adds a substantial dependency, MSRV, and cross-compile surface to the
lightest of the four libraries. (TLS itself is fine — `rustls` is already in the lock file via
`rumqttc`.)

**4.4 The k8s deployment model has no component addressing today.** `templates/*/k8s/` ships
**Deployment + ConfigMap only** — there is no `kind: Service` in any template. The only Services in
the tree are in `test-infra/k8s/chart/`, and they exist for Prometheus scrape, not for
component-to-component addressing. Components today are anonymous pods that talk to a broker; nothing
addresses them. An HTTP transport requires per-component Services, a DNS naming scheme, and a mapping
from UNS identity (`{device}/{component}/{instance}`) to a Service name — a new deployment-model
concern that lands on `deployment-studio`/`ec-deploy` as well as the templates.

**4.5 `subscribeAcknowledged` only means something if a hub exists.** `publishConfirmed` maps
cleanly and arguably better than MQTT — HTTP `2xx` is a genuine end-to-end acknowledgement, where
PUBACK is only broker-receipt. But `subscribeAcknowledged` over HTTP means "the hub accepted my
subscription registration," which presupposes 4.1. Both methods are contractually required to
**throw rather than silently degrade** when a provider cannot prove acknowledgement
([`MessagingProvider.java:187-202, 247-255`](../../libs/java/src/main/java/com/mbreissi/edgecommons/messaging/MessagingProvider.java)),
so a partial HTTP provider must throw, loudly and by design.

**4.6 The parity and interop bill dominates the provider code.** Per
[`CLAUDE.md`](../../CLAUDE.md), a new transport is a wire-reachable change: the `test-infra/interop/`
matrix must be extended so **all four languages produce and consume over HTTP**, on top of per-language
suites at the 92% bundle / 95% diff coverage gates. Provider code is the small part.

---

## 5. What is actually load-bearing — the subscription census

Worth stating explicitly, because it is what makes Option A below viable. The **library's own**
subscriptions are few, and almost all of them are *self-addressed*:

| Subscriber | Filter | Shape |
|---|---|---|
| `CommandInbox` ([`CommandInbox.java:995-998`](../../libs/java/src/main/java/com/mbreissi/edgecommons/commands/CommandInbox.java)) | `ecv1/{device}/{component}/+/cmd/#` and `ecv1/{device}/{component}/cmd/#` | **Addressed.** The wildcards sit in the *instance* and *channel* slots — this is "messages addressed to me," not "everyone's messages." An inbound HTTP endpoint models it exactly. |
| `ConfigComponentProvider` ([`:112`](../../libs/java/src/main/java/com/mbreissi/edgecommons/config/provider/ConfigComponentProvider.java)) | its own `setConfig` topic | **Addressed**, but *server-initiated push* — needs a long-lived stream or a webhook callback. |
| `request()` reply subs | ephemeral per-request reply topic | **Addressed**, one-shot. Native HTTP response. |
| `RepublishListener` ([`:82-88`](../../libs/java/src/main/java/com/mbreissi/edgecommons/uns/RepublishListener.java)) | `ecv1/{device}/_bcast/cmd/republish-{state,cfg}` | **True 1→N broadcast** — the one genuine fan-out inside the library. Fire-and-forget, no `replyTo`. |

The genuine wildcard consumers live **outside** the library: `uns-bridge` subscribes twelve filters
covering both D-U28 scopes (`uns-bridge/src/io.rs:1277-1282`, `relay.rs:154-158`), and `edge-console`
and `telemetry-processor` consume the site UNS the same way.

**Implication:** a component's *inbound* surface is almost entirely addressed and HTTP-shaped. Its
*outbound* surface is almost entirely fan-out and broker-shaped. That asymmetry is what makes a
partial adoption coherent rather than half-baked.

---

## 6. Options

### Option A — HTTP as an *addressed* surface (request/reply + command inbox)

Add HTTP alongside the bus rather than replacing it. `request`/`reply` and the `CommandInbox` become
reachable over HTTP; `publish`/`subscribe` stay on MQTT. A component gets an inbound HTTP endpoint and
an HTTP client for outbound calls.

- **Serves the stated need directly** — "k8s microservices call each other over HTTP" is *exactly*
  the addressed half.
- Does not touch the UNS model, the envelope, or the topic grammar.
- Honest limitation to state in the docs: with HTTP alone there is no `publish`/`subscribe`.
- Precedent already in the product: `edge-console` is the sole browser↔bus bridge and does something
  structurally similar for browsers.
- **Cost: moderate. Risk: low.**

### Option B — full HTTP channel-1 transport with a hub (`--transport HTTP`)

A new `ec-hub` service: HTTP ingest (`POST`), SSE or WebSocket fan-out, subscription registry,
wildcard matching, backpressure.

- Delivers the real k8s-shop wins: service-mesh mTLS (Istio/Linkerd), standard ingress, OTel HTTP
  tracing, NetworkPolicy, no MQTT *technology* to justify to a platform team.
- But you now build, test, secure, scale, and operate a broker. Against EMQX — which is already
  deployed, HA, and battle-tested — that is a hard trade to win.
- Every gate in the validation matrix applies four ways, plus a new component repo.
- **Cost: very high. Risk: high.**

### Option C — sidecar / mesh-native (no core change at all)

The component keeps MQTT to a `localhost` sidecar; the sidecar speaks HTTP to its peers. Purely a
deployment concern.

- **Zero library change, zero parity cost, zero interop cost.**
- Pods communicate over HTTP on the wire, which is usually what a platform mandate actually requires.
- Costs a sidecar per pod and a sidecar to maintain.
- **Cost: low. Risk: low.** Highest value-per-effort if the driver is "HTTP between pods."

### Option D — gRPC instead of REST-style HTTP

Flagged because it may be the strongest technical answer and is easy to miss when the question is
phrased as "HTTP."

- gRPC is HTTP/2 — it satisfies "HTTP-based, mesh-observable, ingress-friendly, no MQTT" just as well.
- **It has native server-streaming**, which is a real `subscribe`, not an SSE approximation.
- **The envelope is already protobuf** — the impedance is near zero, where a REST-style transport
  would base64 protobuf into SSE frames.
- It still needs a rendezvous point for fan-out (4.1 is protocol-independent), but every other
  mismatch shrinks.
- If the underlying driver is a platform mandate rather than REST specifically, this deserves
  evaluation before Option B.

---

## 7. Recommendation

1. **Option C now** if the goal is "HTTP on the wire between pods." It needs no core change and no
   parity spend, and it should be ruled in or out before anything else is built.
2. **Option A** as the first core investment. It is the honest, bounded subset: it serves real
   HTTP microservice interop, it does not pretend to replace the bus, and it cannot quietly erode the
   UNS decoupling.
3. **Option B only against a concrete requirement to eliminate the broker** — and with the
   "we are writing a broker" cost stated and accepted in advance, not discovered during
   implementation.
4. **Evaluate Option D before B.** If the driver is a platform mandate rather than REST semantics,
   gRPC matches the existing protobuf envelope and gives a real streaming `subscribe`.

**What should not happen:** shipping `--transport HTTP` as an implied peer of `--transport MQTT`.
The `Transport` enum reads as a list of interchangeable options; a value that silently supports half
the `MessagingProvider` contract would violate the "throw rather than degrade" discipline the
confirmed-publish and acknowledged-subscribe methods already encode. If Option A proceeds, the naming
and the CLI surface must make the addressed-only scope explicit rather than hiding it behind a
same-shaped flag.

---

## 8. Effort sizing (for Option B, the full case)

Anchored on the existing MQTT providers: Java 934 LOC, Rust 1377 LOC, TS 285 LOC (thinner because it
delegates to a `BrokerChannel`).

| Work | Notes |
|---|---|
| HTTP provider × 4 languages | Comparable to or larger than the MQTT providers — a server *and* a client per language, plus TLS. |
| Rust HTTP stack decision | New dependency surface on the deliberately-light library (4.3). |
| `Transport` enum + resolver + `validate()` + CLI parse × 4 | Mechanical; sites mapped in [`DESIGN-core.md`](DESIGN-core.md) §4.2. New error code — `EC2001` is taken ([`DESIGN-cli.md:313`](DESIGN-cli.md)). |
| Schema | Additive `messaging.http` sibling to `local`/`northbound`/`requestTimeoutSeconds`; single source + `sync-schema`. |
| `ec-hub` component | New repo, new registry entry, HA/backpressure/security design. |
| k8s templates + Services + DNS naming | 16 templates; plus `deployment-studio`/`ec-deploy` impact (4.4). |
| Interop matrix | Four-way produce/consume over HTTP in `test-infra/interop/`. |
| Coverage | 92% bundle / 95% diff, four languages. |
| Docs | This design accepted, per-language docs, website sync. |

Option A is a meaningful fraction of this — no hub, no Services requirement, no fan-out semantics —
but it still carries the four-way parity and interop obligations.

---

## 9. Open questions — these need a decision before any design proceeds

1. **What is the actual driver?** "No broker to operate," a platform mandate for HTTP/mesh
   observability, or specific services that genuinely want RPC? Each points at a different option,
   and Option C answers two of the three with no core change.
2. **Must the UNS survive on the HTTP path?** If yes, a rendezvous point is unavoidable (4.1) and
   Option B is the only complete answer. If HTTP is only for addressed traffic, Option A suffices.
3. **Is a self-maintained hub acceptable versus EMQX?** This is the crux of Option B and is an
   operational question, not a technical one.
4. **REST specifically, or HTTP-based generally?** If the latter, Option D changes the calculus
   substantially.
5. **Does HOST need this at all?** HOST already runs a local broker comfortably. The pressure is
   almost entirely a Kubernetes-shop concern, and scoping to KUBERNETES would cut the matrix.
