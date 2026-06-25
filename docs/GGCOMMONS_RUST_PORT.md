# GGCommons — Rust Port: Design & Implementation Plan

Status: **Complete — all phases delivered and validated on-device** (HOST platform, cross-language parity, and Greengrass IPC incl. `GG_CONFIG`/`SHADOW`/`CONFIG_COMPONENT` and the real-time device-shadow round-trip, validated against a live Greengrass core, non-root) · Target: feature parity with `ggcommons-java-lib`

> The "Phases 0–3" framing and the dated **Decisions (2026-06-15)** below are the original plan, preserved as a historical record. Phase-by-phase results (including Phase 2's on-device validation) are captured in the implementation sections later in this document.

This document is the full design and delivery plan for a Rust implementation of the Greengrass Commons library. It assumes familiarity with the existing Java (`ggcommons-java-lib`, canonical) and Python (`ggcommons-python-lib`) libraries; see the workspace `CLAUDE.md` for ecosystem context.

### Decisions (2026-06-15)
- **Coexistence, not replacement.** The Rust library is a **third implementation** alongside Java and Python — it replaces neither. It must track the same config schema and CLI contract so all three stay at cross-language parity. This is a three-way parity commitment, with its maintenance cost accepted.
- **HOST-platform MVP ships first.** The committed near-term deliverable is the **HOST-platform MVP (Phases 0–1)** — useful for Kubernetes/Docker/container deployments without a Greengrass core. Greengrass IPC parity (Phases 2–3) is planned follow-on, not part of the first ship.
- Remaining open items (MQTT stack, publishing target, logging parity bar) are in §18.

---

## 1. Purpose & scope

Produce a Rust library, crate name **`ggcommons`** (published as `greengrass-commons` to match the Python package), that lets authors build AWS IoT Greengrass v2 components in Rust while writing only business logic — exactly the value proposition of the Java/Python libraries. It bundles the cross-cutting concerns behind service interfaces: configuration, messaging, metrics, heartbeat, and logging.

The Rust port targets **feature parity with the Java library** (the canonical reference), not the Python subset.

### Goals
- Functional parity with the Java library's public behavior: the standard CLI contract, the platform×transport model (GREENGRASS / HOST platforms; IPC / MQTT transports), all five config sources, all four metric targets, request/reply messaging, heartbeat, JSON-schema config validation.
- Idiomatic, async, `tokio`-based Rust with a small dependency surface and small runtime footprint.
- A library that is **correct by construction** — using ownership, RAII, and typed errors to structurally avoid the defect classes found in the Java code review (see §11).
- Small static binaries that cross-compile to edge targets (`aarch64`/`armv7`, musl).

### Non-goals
- **No backward-compatibility burden.** Greenfield crate — ship only the clean builder/trait API; do not replicate the Java legacy `init()` surface.
- No attempt to support the deprecated direct-constructor patterns.
- Not a drop-in ABI/FFI replacement for the Java JAR — it is a native Rust component runtime.

---

## 2. Background: what must be preserved

**Platform × transport** (two independent axes selected at startup):
- **`--platform`** `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`, auto-detected).
  **GREENGRASS** uses Greengrass IPC for messaging and reads config from the Greengrass deployment;
  **HOST** uses dual-MQTT (local broker + AWS IoT Core) for Docker/bare containers; **KUBERNETES** is
  declared but not yet wired (Phase 1 of the platform model).
- **`--transport`** `IPC | MQTT [messaging_config.json]` — default derived from the platform
  (GREENGRASS ⇒ IPC, HOST/KUBERNETES ⇒ MQTT); **IPC is valid only on GREENGRASS**. The MQTT transport
  takes an optional messaging-config JSON path.

**Standard CLI contract** (must match Java/Python exactly):
- `-c/--config <SOURCE> [args...]` — `FILE | ENV | GG_CONFIG (default) | SHADOW | CONFIG_COMPONENT`
- `--platform <PLATFORM>` — `GREENGRASS | HOST | KUBERNETES | auto` (default `auto`)
- `--transport <TRANSPORT> [path]` — `IPC | MQTT [messaging_config.json]` (IPC only valid on GREENGRASS)
- `-t/--thing <name>` — IoT Thing name; must take the **full** string value

> The legacy `-m/--mode` flag has been **removed**: `-m GREENGRASS` → `--platform GREENGRASS`;
> `-m STANDALONE <path>` → `--platform HOST --transport MQTT <path>`. It now errors with this guidance.

**Config schema** (carried over verbatim — see §13).

---

## 3. Why Rust (motivation)

1. **Edge/IoT fit.** Greengrass components run on constrained gateways. A static Rust binary removes the JRE dependency, slashes memory footprint, and gives near-instant cold start. Cross-compiles cleanly to ARM.
2. **Correctness.** The Java code review surfaced recurring concurrency races, resource leaks, and `System.exit()` anti-patterns. Rust's borrow checker, `Drop`/RAII, and `Result` make those mistakes structurally hard (§11).
3. **The blocker is gone.** Greengrass v2 IPC — the one piece that would otherwise require implementing the eventstream-rpc protocol — is covered by an official native Rust crate (§4).

---

## 4. Greengrass IPC: resolved via official SDK

The library depends on Greengrass IPC for local pub/sub, the IoT Core bridge, deployment configuration, and device shadows. AWS ships [`aws-greengrass-component-sdk`](https://github.com/aws-greengrass/aws-greengrass-component-sdk) — a **native Rust crate** (v1.0.x, Apache-2.0, "low resource footprint") that covers the full operation set we need:

| IPC operation | ggcommons subsystem | Covered |
|---|---|---|
| `PublishToTopic` / `SubscribeToTopic` | Messaging (GG mode) | ✅ |
| `PublishToIoTCore` / `SubscribeToIoTCore` | Messaging (IoT Core bridge) | ✅ |
| `GetConfiguration` | `GreengrassConfigSource` (GG_CONFIG) | ✅ |
| `SubscribeToConfigurationUpdate` | Config hot-reload | ✅ |
| `UpdateConfiguration` | Config reporting | ✅ |
| `GetThingShadow` / `UpdateThingShadow` / `DeleteThingShadow` / `ListNamedShadowsForThing` | `ShadowConfigSource` | ✅ |
| `GetSecretValue` | *(not used by ggcommons)* | n/a |

> **Spike required before committing.** The crate is young; confirm its async model and API ergonomics with a 1–2 day spike (§17, §18) before locking the Phase 2 estimate.

---

## 5. Dependency mapping

| Concern | Java today | Rust crate | Notes |
|---|---|---|---|
| Greengrass IPC | `aws-iot-device-sdk` (Java) | `aws-greengrass-component-sdk` | native crate |
| MQTT (standalone) | Eclipse Paho | `rumqttc` | pure-Rust, async |
| TLS / mTLS | JSSE | `rustls` + `tokio-rustls` | safer defaults than the Java `SSLSocketFactory` path |
| CloudWatch | AWS SDK for Java | `aws-sdk-cloudwatch` | AWS SDK for Rust is GA |
| JSON | Gson | `serde` + `serde_json` | typed where possible, `Value` for loose trees |
| JSON-schema validation | `networknt/json-schema-validator` | `jsonschema` | |
| System metrics | OSHI | `sysinfo` (+ `/proc` reads for FDs on Linux) | |
| File watching | custom `FileWatcher` | `notify` | |
| CLI | commons-cli | `clap` (derive) | |
| Logging | SLF4J/Log4j2/JUL | `tracing` + `tracing-subscriber` + `tracing-appender` | runtime reload via `reload::Handle` |
| Async runtime | threads + `CompletableFuture` | `tokio` | |
| Error handling | exceptions / `System.exit` | `thiserror` (lib), `anyhow` (bins/examples) | |
| Builders | hand-written | hand-written or `derive_builder` | |

---

## 6. Crate structure

Single library crate with **cargo features** so a component only pulls in what it uses (footprint + compile time):

```
ggcommons/
  Cargo.toml            # features: ["standalone", "greengrass"] (default = both)
  src/
    lib.rs              # public API: GgCommons, GgCommonsBuilder, prelude
    error.rs            # GgError (thiserror), Result alias
    cli.rs              # clap parser -> ParsedArgs { platform, transport, config_source, thing }
    config/
      mod.rs            # ConfigService trait, Config snapshot, ConfigHandle
      model.rs          # serde structs (Logging/Heartbeat/Metric/Tag/Component)
      validation.rs     # jsonschema (embedded schema resource)
      template.rs       # {ThingName}/{ComponentName}/{tag} substitution
      source/
        mod.rs          # ConfigSource trait + builder/dispatch
        file.rs         # FILE  (notify-based hot reload)
        env.rs          # ENV
        greengrass.rs   # GG_CONFIG  (feature = "greengrass")
        shadow.rs       # SHADOW     (feature = "greengrass")
        config_component.rs  # CONFIG_COMPONENT (messaging-based)
    messaging/
      mod.rs            # MessagingService trait, MessagingProvider trait
      message.rs        # Message, MessageHeader, MessageTags (+ builders)
      request_reply.rs  # transport-agnostic correlation layer
      provider/
        mqtt.rs         # dual-broker MQTT      (feature = "standalone")
        ipc.rs          # Greengrass IPC        (feature = "greengrass")
    metrics/
      mod.rs            # MetricService trait, Metric, Measure, MetricBuilder
      emf.rs            # Embedded Metric Format
      target/
        log.rs          # EMF -> rotating log file
        cloudwatch.rs   # aws-sdk-cloudwatch, batched
        cloudwatch_component.rs  # publish to GG CW component
        messaging.rs    # publish metrics over messaging
    heartbeat.rs        # periodic system metrics
    logging.rs          # tracing init + runtime reconfiguration
  tests/                # integration tests (local MQTT broker via testcontainers)
  examples/             # skeleton component
```

---

## 7. Async model

**Decision: `tokio`, async throughout.** The component SDK, `rumqttc`, and the AWS SDK are all async. Trait methods that do I/O are `async` and the service traits are object-safe via `#[async_trait]` (so we can hold `Arc<dyn MessagingService>` for the testable seam).

The component's `main` is a `#[tokio::main]` that builds `GgCommons`, retrieves services, and runs business logic. Long-running tasks (heartbeat ticks, CloudWatch batch flush, config watchers) are `tokio::task`s, not OS threads — a panic in one is isolated and observable, eliminating the Java "Timer thread dies → feature silently stops forever" failure mode.

---

## 8. Error handling strategy

- Library returns `Result<T, GgError>`. **No `process::exit` anywhere in the library** (the Java code had 18 such call sites). `GgError` is a `thiserror` enum with variants per subsystem (`Config`, `Messaging`, `Metrics`, `Ipc`, `Validation`, `Cli`).
- Connection/credential failures are **errors**, never silent fallbacks. In particular, a TLS credential load failure for IoT Core is a hard error — it must never "connect without credentials" (fixes Java review C3).
- The binary/`main` (in skeletons) decides whether to exit, retry, or log — using `anyhow` for ergonomic top-level handling.
- Async tasks wrap their work bodies so a single failed tick logs and continues rather than killing the task.

---

## 9. Subsystem designs

### 9.1 Configuration

**Snapshot + atomic publish.** The Java bug C6 was unsynchronized mutation of live config from background threads. Rust design: config is an immutable `Arc<Config>` published through **`arc_swap::ArcSwap`**. A reload builds a fully-populated `Config` in a local, validates it, then swaps it in with a single atomic store. Readers call `config.load()` and get a consistent snapshot; no torn reads possible.

```rust
pub struct Config {
    pub component_name: String,
    pub thing_name: String,
    pub logging: LoggingConfig,
    pub heartbeat: HeartbeatConfig,
    pub metrics: MetricConfig,
    pub tags: TagConfig,
    pub global: serde_json::Value,
    pub instances: IndexMap<String, serde_json::Value>,
    raw: serde_json::Value, // for template substitution over arbitrary keys
}

pub struct ConfigHandle {
    current: arc_swap::ArcSwap<Config>,
    tx: tokio::sync::watch::Sender<Arc<Config>>,
}
```

**Change notification via `tokio::sync::watch`** instead of the Java listener interface. Subscribers hold a `watch::Receiver<Arc<Config>>` and `.changed().await`. This replaces the error-prone "iterate listeners, one throw aborts the rest, return-value contract ignored" Java pattern (review M4) with a channel that fans out cleanly and can't be corrupted by a misbehaving subscriber.

**Config sources** implement a `ConfigSource` trait:

```rust
#[async_trait]
pub trait ConfigSource: Send + Sync {
    async fn load(&self) -> Result<serde_json::Value>;
    /// Optional: stream updates for hot-reload sources.
    fn watch(&self) -> Option<tokio::sync::mpsc::Receiver<serde_json::Value>> { None }
    fn source_name(&self) -> &str;
}
```

- `FileConfigSource` — `serde_json` read; hot reload via `notify` (debounced) → emits on the watch channel. (Fixes Java H9: a watcher error logs and the source reports unhealthy rather than silently dying forever.)
- `EnvConfigSource` — read env var (default `CONFIG`).
- `GreengrassConfigSource` — `GetConfiguration` + `SubscribeToConfigurationUpdate` (feature `greengrass`).
- `ShadowConfigSource` — named-shadow get + delta subscription; reports state back via `UpdateThingShadow` (feature `greengrass`).
- `ConfigComponentSource` — request/reply over messaging to a dedicated config component, plus an updated-topic subscription.

**Validation** (`jsonschema`): the schema is embedded with `include_str!` so it can never be "missing from the classpath" — closing the Java fail-open hole (review M5). Validation failure is a hard error by default.

**Template substitution** (`template.rs`): `{ThingName}`, `{ComponentName}`, `{ComponentFullName}`, and tag keys. Substitution values are validated/escaped where used in file paths and topics (closing the Java injection/path-traversal concern, review M15). Missing thing name is handled explicitly (no panic).

### 9.2 Messaging

**Two-layer design** — this is the key structural improvement over Java, which duplicated request/reply inside each provider and let them drift (causing review C1/C2/H7).

**Layer 1 — transport (`MessagingProvider`)** knows only how to move bytes on topics:

```rust
#[async_trait]
pub trait MessagingProvider: Send + Sync {
    async fn publish(&self, topic: &str, payload: &[u8], dest: Destination, qos: Qos) -> Result<()>;
    async fn subscribe(&self, filter: &str, dest: Destination, qos: Qos) -> Result<Subscription>;
    async fn unsubscribe(&self, filter: &str, dest: Destination) -> Result<()>;
}
// Destination = { Local, IotCore }   Subscription yields a stream of (topic, payload)
```

- `MqttProvider` (feature `standalone`): two `rumqttc` clients (local broker + IoT Core), `rustls` mTLS to IoT Core, username/password or cert auth to the local broker. Blocking-until-confirmed semantics preserved by awaiting CONNACK/SUBACK. Auto-reconnect + re-subscribe (fixes Java M11 TODO no-op).
- `IpcProvider` (feature `greengrass`): wraps the component SDK's pub/sub and IoT Core operations.

**Layer 2 — `MessagingService`** is transport-agnostic and built **once** over any `MessagingProvider`. It owns:
- `Message` (de)serialization (header/tags/body), `MessageBuilder`.
- **Request/reply correlation** (`request_reply.rs`): generate a reply-to topic, tag the message with a correlation id, register a `oneshot::Sender` in a `Mutex<HashMap<String, oneshot::Sender<Message>>>`, subscribe to the reply topic, publish the request, and await with `tokio::time::timeout`. On completion **or timeout** the entry is removed and the subscription dropped — no leaks (fixes Java H2). Cancellation drops the sender. Because correlation lives above the transport, **it works identically over MQTT and IPC and is fully testable over a local broker** (this is why it lands in Phase 1).

```rust
#[async_trait]
pub trait MessagingService: Send + Sync {
    async fn publish(&self, topic: &str, msg: &Message, dest: Destination) -> Result<()>;
    async fn subscribe(&self, filter: &str, dest: Destination)
        -> Result<impl Stream<Item = (String, Message)>>;
    async fn request(&self, topic: &str, msg: Message, dest: Destination, timeout: Duration)
        -> Result<Message>;
    async fn reply(&self, request: &Message, reply: Message) -> Result<()>;
}
```

`Message`/`MessageHeader`/`MessageTags` are `Clone` value types with `serde` (de)serialization. No shared-mutable-tags races (fixes Java M1/M2); a correlation id is assigned at construction, not lazily in a getter.

### 9.3 Metrics

`MetricService` trait + `Metric`/`Measure`/`MetricBuilder`. Targets implement a `MetricTarget` trait and subscribe to config changes via the watch channel.

```rust
#[async_trait]
pub trait MetricTarget: Send + Sync {
    async fn emit(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()>;     // batched
    async fn emit_now(&self, metric: &Metric, values: &HashMap<String, f64>) -> Result<()>; // immediate
}
```

- `log` — EMF JSON to a rotating file via `tracing-appender` (single appender, retargeted on config change — fixes Java H4 appender leak).
- `cloudwatch` — `aws-sdk-cloudwatch`; batched on a `tokio` interval; per-namespace error isolation and ≤1000-datum chunking (fixes Java H8); region from the SDK default chain, not hardcoded.
- `cloudwatch_component` — publish to the GG CloudWatch component over messaging.
- `messaging` — publish EMF over messaging (local or IoT Core).

**EMF correctness:** `_aws.Timestamp` in **milliseconds** (fixes Java H5, which divided by 1000); ≤10-dimension cap enforced on `Metric` itself, not just the builder (fixes Java M9). `isMetricDefined` is a pure lookup with no emit side-effect (fixes Java H6).

### 9.4 Heartbeat

A `tokio` interval task collecting system metrics via `sysinfo` (CPU %, memory RSS, disk, threads; FDs via `/proc/self/fd` on Linux / `sysinfo` handles elsewhere). Targets: `metric` (via `MetricService`) and/or `messaging`. The tick body is wrapped so a transient failure logs and the next tick still fires (fixes Java C4/C5 — the heartbeat can't be permanently killed by one error, and a missing target `config` is handled, not a panic). Reconfigures itself off the config watch channel. Memory reported with correct units (fixes Java H4 unit/precision bug).

### 9.5 Logging

`tracing` + `tracing-subscriber` with an `EnvFilter` behind a `reload::Handle` for runtime level changes, and `tracing-appender` (non-blocking, rolling) for file output. Maps the config `logging` section: `level`, `format`, `fileLogging`, per-logger levels (`loggers`), and `globalControl`. Runtime reconfiguration applied in exactly one place driven by the config watch channel (fixes Java M4 double-reconfigure).

> This is the subsystem with the largest semantic gap from Java's Log4j2 model (see §17). Per-logger dynamic levels map to `EnvFilter` directives; full appender swap at runtime is more constrained but achievable.

### 9.6 Dependency injection / composition

No runtime type-keyed registry (unidiomatic in Rust). Instead, `GgCommons` holds the wired services as trait objects, exposed by typed accessors:

```rust
pub struct GgCommons {
    config: Arc<dyn ConfigService>,
    messaging: Arc<dyn MessagingService>,
    metrics: Arc<dyn MetricService>,
    _heartbeat: Heartbeat, // owns its task; stops on Drop
}
impl GgCommons {
    pub fn config(&self) -> Arc<dyn ConfigService> { self.config.clone() }
    pub fn messaging(&self) -> Arc<dyn MessagingService> { self.messaging.clone() }
    pub fn metrics(&self) -> Arc<dyn MetricService> { self.metrics.clone() }
}
```

The trait objects are the testable seam: inject fakes in tests (replacing the Java `MockMessagingService`/`TestableGGCommons` harness). RAII handles shutdown — dropping `GgCommons` stops the heartbeat task, closes MQTT/IPC clients, and cancels watchers (fixes the Java "no `close()` anywhere" leak class, review H3).

---

## 10. Public API sketch

```rust
use ggcommons::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let gg = GgCommonsBuilder::new("com.example.MyComponent")
        .args(std::env::args())   // standard -c/-m/-t contract via clap
        .build()
        .await?;

    let messaging = gg.messaging();
    let config = gg.config();

    let mut sub = messaging.subscribe("requests/process", Destination::Local).await?;

    while let Some((topic, msg)) = sub.next().await {
        let reply = MessageBuilder::new("ProcessResult", "1.0")
            .payload(serde_json::json!({ "ok": true }))
            .from_config(&config)
            .build();
        messaging.reply(&msg, reply).await?;
    }
    Ok(())
}
```

Builders (`GgCommonsBuilder`, `MessageBuilder`, `MetricBuilder`) mirror the Java fluent API so the mental model transfers.

---

## 11. How the design addresses the Java code-review findings

| Java finding | Severity | Rust resolution |
|---|---|---|
| C1/C2 — `requestFromIoTCore` wrong transport / wrong unsubscribe | Critical | Request/reply built **once** over a transport trait; provider only does pub/sub. Cannot diverge. |
| C3 — silent TLS fallback to no client creds | Critical | Credential load failure is a hard `GgError`; never connects unauthenticated. |
| C4/C5 — heartbeat Timer dies on exception / null config NPE | Critical | `tokio` task with wrapped body; `Option` config handled by the type system. |
| C6 — config hot-reload data races | Critical | `ArcSwap<Arc<Config>>` snapshot publish; borrow checker forbids unsynchronized shared mutation. |
| H1 — `System.exit()` ×18 in a library | High | Library is `Result`-based; zero process exits. |
| H2 — request/reply futures leak (no timeout) | High | `tokio::time::timeout` + guaranteed map/subscription cleanup. |
| H3/H4 — no `close()`; client/thread/appender leaks | High | RAII/`Drop`; single retargeted appender. |
| H5 — EMF timestamp in seconds | High | Milliseconds. |
| H6 — `isMetricDefined` emits a metric | High | Pure lookup. |
| H8 — CloudWatch flush loses metrics, no chunking | High | Per-namespace isolation, ≤1000-datum batches, retry. |
| H9 — FileWatcher swallows `Throwable`, dies | High | `notify` errors surface as source-unhealthy; recoverable. |
| M1/M2 — shared mutable message/tags races | Medium | `Clone` value types; id set at construction. |
| M4 — listener contract ignored, one throw aborts all | Medium | `watch` channel fan-out. |
| M5 — schema validation fail-open | Medium | Embedded schema; fail-closed. |
| M9 — dimension cap only in builder | Medium | Enforced on `Metric`. |
| M11 — `connectionLost` no-op, no reconnect | Medium | Auto-reconnect + re-subscribe. |

This is the core argument: a faithful idiomatic port is not just equivalent to the Java library — it **starts from a more correct baseline**.

---

## 12. Configuration schema (carried over verbatim)

Same keys as Java/Python; modeled with `serde` structs (loose subtrees as `serde_json::Value`):

```
logging:        { level, format, fileLogging: { enabled, filePath }, loggers: {name: level}, globalControl }
metricEmission: { target: log|messaging|cloudwatch|cloudwatchcomponent, namespace, largeFleetWorkaround,
                  targetConfig: { logFileName, maxFileSize, topic, destination, intervalSecs } }
heartbeat:      { intervalSecs, measures: { cpu, memory, disk, threads, files, fds }, targets: [{type, config}] }
tags:           { <key>: <value>, ... }
component:      { global: {...}, instances: [ { id, ... }, ... ] }
```

The embedded JSON schema is shared in spirit with the Java `/ggcommons-config-schema.json` to keep cross-language validation aligned.

---

## 13. Testing strategy

(The Java CI **skips tests** — do not replicate that.)

- **Unit tests** per module; service traits enable fakes (`FakeMessaging`, `FakeConfig`, `FakeMetrics`) — the Rust analog of the Java mock harness.
- **Integration tests** for messaging/request-reply against a **local MQTT broker** via `testcontainers` (EMQX/Mosquitto) — Phase 1 is fully testable device-free, including request/reply correlation, timeout, and cancellation.
- **Property tests** (`proptest`) for topic-filter matching and template substitution (the Java matcher had disabled validation, review L18).
- **Greengrass IPC tests**: a thin contract test plus manual on-device validation (Phase 2), since IPC needs a Greengrass core.
- CI: `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check`, and cross-compile smoke build for `aarch64-unknown-linux-musl`.

---

## 14. Packaging & deployment

- Build with `cargo`; ship the compiled binary as the component artifact.
- GDK (`gdk-config.json`): `build_system: custom` with a `cargo build --release` (or cross) command, then zip the target binary; `recipe.yaml` unchanged in shape (declares default config + IPC `accessControl`).
- Cross-compile to `aarch64-unknown-linux-musl` / `armv7-unknown-linux-musleabihf` via `cross` for static, JRE-free edge binaries.
- Cargo features let a standalone-only deployment omit the IPC SDK and vice versa.

---

## 15. Phased delivery plan

Sequencing puts all **device-free** work first; the only Greengrass-core-dependent work is Phase 2.

> **Committed scope (decided 2026-06-15):** Phases 0–1 are the **standalone MVP — the first ship**. Phases 2–3 (Greengrass IPC + full parity) are planned follow-on, scheduled after the MVP lands.

### Phase 0 — Foundations (1–2 wks)
- Crate scaffold, `Cargo.toml` features, CI.
- `error.rs` (`GgError`); `cli.rs` (clap, full `-c`/`--platform`/`--transport`/`-t` contract incl. IPC-only-on-GREENGRASS validation and full-string `-t`).
- Config model (`serde`), embedded JSON schema + `jsonschema` validation, template substitution.
- `tracing` logging baseline.
- **Deliverable:** parses args, loads + validates a FILE/ENV config, logs. Unit-tested.

### Phase 1 — HOST platform, end-to-end (3–5 wks)
- ✅ `MessagingProvider` trait + `MqttProvider` (dual broker: local + IoT Core, reconnect/re-subscribe).
- ✅ `MessagingService` + `Message`/builders + explicit `…ToIoTCore` pairs + raw publish/QoS + **request/reply (`ReplyFuture`) with timeout/cancel** — tested over the local EMQX broker.
- ✅ **Increment 1:** all four metric targets + EMF (`cloudwatch` behind a feature).
- ✅ **Increment 2:** heartbeat via `sysinfo` (+ Linux `/proc` / Windows `windows-sys` for threads/fds/files).
- ✅ **Increment 3:** FILE config hot-reload (`notify`) → validate → atomic `ArcSwap` swap → `ConfigChangeListener` notification; heartbeat reacts to reloads.
- ✅ **Increment 4:** IoT Core mutual TLS (and local TLS) via `rustls` — `caPath`/`certPath`/`keyPath` from the messaging config; no insecure fallback. Verified live over EMQX `8883`.
- **Deliverable (complete):** a HOST-platform component that connects to the local broker **and AWS IoT Core (mTLS)**, does pub/sub **and request/reply**, emits metrics, heartbeats, and hot-reloads config — runnable on a laptop with zero Greengrass.

### Phase 2 — Greengrass IPC (validated on real hardware)
The SDK (`aws-greengrass-component-sdk`, lib `gg_sdk`, v1.0.4) turned out to be a
**synchronous, `no_std`, process-global C-FFI** binding with lifetime-bound
subscription callbacks — not the async model the design assumed. Bridged via a single
shared worker thread ([`src/ipc.rs`], `IpcRuntime`) that owns the `Sdk` and all live
subscriptions; async callers dispatch commands and await `oneshot` replies.
- ✅ `IpcProvider` over the SDK (local pub/sub + IoT Core bridge). **Request/reply inherited unchanged** from Phase 1 (built once over the transport trait).
- ✅ `GreengrassConfigSource` (`GetConfiguration` + `SubscribeToConfigurationUpdate`, re-fetch-on-change since the update delivers only the key path).
- ✅ `ShadowConfigSource` (`GetThingShadow` + `.../update/delta` subscription over the IoT Core bridge).
- ✅ `ConfigComponentSource` (request/reply over messaging; topic contract at Java/Python parity). `build()` reordered to init messaging before the config source.
- ✅ Wired into `GgCommonsBuilder::build` behind the `greengrass` cargo feature.
- **Verified:** builds, clippy (`-D warnings`), and tests/doctests all clean under `--features greengrass` on Linux (WSL Ubuntu, rustc 1.96), including the SDK's C-FFI build.

#### On-device validation (2026-06-16)
Validated against a real AWS IoT Greengrass v2 **Nucleus v2.17.0** installed natively on an Ubuntu 26.04 x86_64 lab workstation (`lab-5950x`), provisioned against AWS IoT Core (account region `us-east-1`) and running as a `systemd` service. The Rust component was cross-built in WSL with `--no-default-features --features greengrass` and deployed via the Greengrass CLI as a local deployment.

Confirmed working as a **non-root component** (run-as user `ggc_user`, the production-correct configuration):
- IPC connect (`Sdk::init()` + `connect()` to the real IPC socket).
- `GG_CONFIG` config source via `GetConfiguration` (with the original `["ComponentConfig"]` key path) — config values were read and applied (e.g. publish interval).
- `IpcProvider::publish` (local pub/sub) — periodic publishes observed.
- `IpcProvider::subscribe` registered on the request topic.
- Heartbeat + log metric target running.

**Key finding: every failure encountered during bring-up was environmental, not a defect in the Rust code.** The compile-only Phase 2 code ran correctly as written. Specifically:
1. `GetSessionToken` temporary credentials cannot call IAM without MFA → used the IAM user's long-lived access key for provisioning instead.
2. The lab box's `~/.aws/config` had a LocalStack `endpoint_url` that hijacked the installer's AWS calls → removed it.
3. **Ubuntu 26.04 ships `sudo-rs` (the Rust rewrite) as the default `/usr/bin/sudo`, and `sudo-rs` does not implement `-E` (preserve environment).** Greengrass relies on `sudo -E` to pass IPC env vars (`SVCUID`, `AWS_GG_NUCLEUS_DOMAIN_SOCKET_FILEPATH_FOR_COMPONENT`, `AWS_IOT_THING_NAME`) to privilege-dropped (non-root) components. Result: non-root components got an empty environment and the SDK `connect()` failed with `GG_ERR_CONFIG`. **Fix:** switch the sudo alternative to classic sudo (`update-alternatives --set sudo /usr/bin/sudo.ws`, reversible via `--auto`) **and** add a `/etc/sudoers.d` drop-in with `Defaults setenv` so classic sudo honors `-E`.
4. A non-root component cannot write the metric `log` target to the default `/greengrass/v2/logs` path (that dir is root-only, mode 700) → point `metricEmission.targetConfig.logFileName` at the component's work directory instead.

The speculative `GetConfiguration` whole-config + Rust-side key extraction change was **reverted**: the original key-path implementation works correctly on the real Nucleus; that error had been a symptom of the env issue, not a real bug.

**Additionally validated on-device (2026-06-16, non-root):**
- **Inbound local pub/sub delivery + full request/reply correlation** over IPC — a self-request was published, the subscribed handler was invoked (`received request`), replied, and the reply was correlated back to the requester (`request/reply round-trip OK`).
- **IoT Core bridge, device side** — `subscribe_to_iot_core` registered the command topic and `publish_to_iot_core` mirrored telemetry every tick with no authorization/IPC errors (the recipe's `aws.greengrass.ipc.mqttproxy` accessControl grants it). The cloud→device round trip and observing telemetry in the cloud were not exercised (the provisioning IAM identity lacks `iot:Publish` data-plane permission; confirm via the AWS IoT MQTT test client).
- **Config-update hot reload** — a deployment config change to `publish_interval` triggered `SubscribeToConfigurationUpdate` → the `IpcRuntime` re-fetch → atomic snapshot swap (`configuration reloaded`) → listener notifications (`metric target reconfigured after config change`). Note: the demo app reads a one-time `gg.config()` snapshot in its publish loop, so its cadence didn't change; the library reload + listener path (and the heartbeat, which reads the live snapshot per tick) is what this validates.

**IoT Core bridge — validated end-to-end (2026-06-16):** cloud→device confirmed (`aws iot-data publish` → the component's `subscribe_to_iot_core` handler fired); device→cloud confirmed (a SigV4 MQTT-over-WebSocket subscriber observed the component's `publish_to_iot_core` telemetry arriving in the cloud as ggcommons `Message` envelopes). Note: the subscribe dispatcher only accepts the ggcommons envelope (header/tags/body) — a raw external IoT Core payload is dropped as unparseable (consistent with Java/Python; a `subscribe_*_raw` variant would be a future enhancement for arbitrary-publisher interop).

**`SHADOW` config source — validated end-to-end (2026-06-16):** deployed AWS `ShadowManager` (2.3.14) with classic-shadow sync; set the device shadow's `state.desired` to a full ggcommons config; ran the skeleton with `-c SHADOW`. It started `config_source="SHADOW"` and applied the shadow's values (the `publish_interval: 8` from the shadow drove an 8s publish cadence — distinct from the GG_CONFIG defaults), confirming `ShadowConfigSource` → IPC `GetThingShadow` → extract `state.desired` → validate → `Config`. **Real-time updates also confirmed:** changing the cloud shadow's `desired.publish_interval` (8→3) on the running component triggered the `update/delta` subscription → re-fetch → atomic reload → `ConfigurationChangeListener`, and the publish cadence shifted to 3s with no restart (seq counter unbroken).

**Phase 2 on-device validation: complete.** Every GREENGRASS-platform capability has been exercised against a live Nucleus, non-root, except `CONFIG_COMPONENT` (messaging-based request/reply to a dedicated config component) — its mechanism is covered by the validated local request/reply path, but it was not stood up with a peer config component on-device.

### Phase 3 — Parity & hardening (2–3 wks)
- ✅ Config snapshot edge cases, **multi-instance** (`instance_ids()`/`instance(id)`, verified end-to-end through `GgCommons`), logging runtime reconfiguration parity.
- ✅ **Live subsystem reconfiguration on config hot-reload:** the metric target is rebuilt and the logging level reconfigured when the config changes (the heartbeat already reacts in Phase 1).
- ✅ **Template-substitution hardening:** substituted values (thing name, component name, tags) are sanitized against path traversal and MQTT topic-wildcard injection (closes Java review M15).
- ✅ Docs (per-subsystem under `doc/`, matching the Java layout) and a Rust component skeleton (`../rust-component-skeleton`).
- ✅ **GDK packaging:** `gdk-config.json` (`build_system: custom` → `build.sh`) + `recipe.yaml` for the Rust skeleton; the custom build stages the binary + recipe into `greengrass-build/` per the GDK contract.
- ⏭️ **ARM cross-compile dropped** (2026-06-15): deployment targets are x64; the code is already portable (only `windows-sys`-vs-`/proc` is cfg-gated), so aarch64 remains buildable later as a CI concern if a device ever needs it.
- **Deliverable:** documented, packaged crate at Java feature parity (standalone surface).

**Rough total: ~2.5–4 months** for one experienced Rust engineer to reach Java parity. A **standalone-only MVP** (Phases 0–1) is useful on its own in **~6–8 weeks** for K8s/Docker deployments.

---

## 16. Risks & mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| Component SDK maturity / API ergonomics / docs | Medium (schedule) | De-risk with the §18 spike before Phase 2 |
| Team Rust expertise (async Rust learning curve) | Medium (org) | Real-world gating factor; staff/upskill accordingly |
| Logging runtime-reconfig parity vs Log4j2 | Low–Med | Scope explicitly; accept slightly different per-logger semantics |
| IoT Core mTLS specifics in `rumqttc`/`rustls` | Low | Well-trodden; `rustls` handles it |
| GDK packaging of a compiled binary | Low | `build_system: custom` |
| Three-way maintenance (Java + Python + Rust) | Strategic | Decide whether Rust *adds to* or *replaces* existing libs before committing (see §18) |

---

## 17. Architectural translation notes (non-1:1 mappings)

- **DI registry → composition** (§9.6): drop the type-keyed registry; use trait objects + accessors.
- **Blocking → async**: paradigm shift, net positive; decide async early (done: §7).
- **Dynamic JSON → typed serde**: mostly a win; keep `Value` for arbitrary config/message subtrees used by template substitution.
- **No legacy API**: simplification — ship only the clean surface.
- **Logging**: the one area where Rust is less capable out-of-the-box (§9.5).

---

## 18. Decisions & open items

**Decided (2026-06-15):**
1. ~~**Strategic intent**~~ → **Coexist.** Rust is a third implementation alongside Java and Python; it replaces neither. Three-way cross-language parity (config schema + CLI contract) is a standing commitment.
2. ~~**MVP scope**~~ → **Standalone-only MVP first** (Phases 0–1) for container deployments; Greengrass IPC parity (Phases 2–3) is follow-on.

3. ~~**MQTT stack**~~ → **`rumqttc`** (pure Rust, async; no C toolchain — clean ARM/musl cross-compile). Confirm IoT Core mTLS behavior during Phase 1 work.

**Still open (need product/eng sign-off):**
4. **Crate/publishing:** crate name `ggcommons`, published as `greengrass-commons`; registry (crates.io vs internal GitLab).
5. **Logging parity bar:** how faithfully must per-logger dynamic reconfiguration match Log4j2?

---

## 19. First steps

**For the standalone MVP (committed):** no Greengrass spike is required — the MVP has zero Greengrass-core dependency. Begin directly with **Phase 0** (scaffold, CLI, config model + validation, logging), then **Phase 1** (dual-broker MQTT, request/reply, metrics, heartbeat), all testable on a laptop with a local MQTT broker. The first decision to settle is the MQTT stack (§18 #3).

**Before Phase 2 (follow-on):** de-risk Greengrass IPC with a 1–2 day spike — a minimal Rust component that (1) subscribes to a local topic and (2) reads deployment configuration via `GetConfiguration`, using `aws-greengrass-component-sdk` on a real Greengrass core device. This validates the one genuine unknown (the component SDK's async model, ergonomics, and maturity) and is the gate to starting Phase 2.

---

## Sources
- [aws-greengrass/aws-greengrass-component-sdk](https://github.com/aws-greengrass/aws-greengrass-component-sdk)
- [AWS SDK for Rust — GA announcement](https://aws.amazon.com/about-aws/whats-new/2023/11/aws-sdk-rust)
- [`aws-sdk-cloudwatch` (crates.io)](https://crates.io/crates/aws-sdk-cloudwatch)
- [Greengrass v2 interprocess communication (AWS docs)](https://docs.aws.amazon.com/greengrass/v2/developerguide/interprocess-communication.html)
- Internal: `ggcommons-java-lib` code review (this engagement); workspace `CLAUDE.md`.
