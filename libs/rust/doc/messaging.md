# Messaging

Messaging is a two-layer design — a deliberate structural improvement over the Java
library, which duplicated request/reply inside each provider and let them drift.

- **Layer 1 — `MessagingProvider`** moves bytes on topics for a `Destination`
  (`Local` or `IotCore`) at a given `Qos`. Implementations: `MqttProvider`
  (the MQTT transport, dual broker) and `IpcProvider` (the IPC transport — Greengrass
  IPC, `greengrass` feature).
- **Layer 2 — `MessagingService`** is transport-agnostic and built **once** over any
  provider. It owns message (de)serialization, the callback dispatch model, and
  request/reply correlation. This is the API component authors use.

Obtain it from the runtime: `let svc = gg.messaging()?;` (returns `Err` only on the
IPC transport — i.e. `--platform GREENGRASS` — when the `greengrass` feature is disabled).

## Explicit local / IoT Core method pairs

Mirroring the Greengrass v2 API and the Java/Python `IMessagingService`, every
operation has an explicit local and IoT Core form (rather than a destination
argument):

| Local | IoT Core |
|-------|----------|
| `publish` | `publish_to_iot_core` (+ `qos`) |
| `publish_raw` | `publish_to_iot_core_raw` (+ `qos`) |
| `subscribe` | `subscribe_to_iot_core` (+ `qos`) |
| `unsubscribe` | `unsubscribe_from_iot_core` |
| `request` / `request_with_timeout` | `request_from_iot_core` / `request_from_iot_core_with_timeout` |
| `reply` | `reply_to_iot_core` |
| `cancel_request` | `cancel_request_from_iot_core` |

(Per the cross-language casing decision D‑U7, Rust keeps the `_iot_core` / `IotCore` spelling —
RFC-430 idiom — where Java/TS renamed to `IoTCore`.)

## UNS topics & the reserved-class guard

Topics follow the Unified Namespace grammar `ecv1/{device}/{component}/{instance}/{class}[/channel]`
(see `docs/platform/DESIGN-uns.md`). Build them with the validating builder rather than by hand:

```rust
use ggcommons::uns::UnsClass;

let topic = gg.uns().topic_with_channel(UnsClass::App, "order/received")?;
// -> ecv1/gw-01/my-component/main/app/order/received
svc.publish(&topic, &msg).await?;

let inst = gg.instance("kep1")?;              // instance-scoped handle
let data = inst.uns().topic_with_channel(UnsClass::Data, "press12/temperature")?;
```

The four **reserved platform classes** — `state`, `metric`, `cfg`, `log` — are library-owned: every
publish-side method (`publish`, `publish_raw`, `request*`, `reply*`, and their IoT Core variants)
rejects a client-chosen reserved-class `ecv1/…` topic with `GgError::ReservedTopic`. Use
`gg.uns()` / the heartbeat/metric subsystems instead; `subscribe*` is never guarded. Non-`ecv1`
topics (the `ggcommons/reply-…` prefix, `cloudwatch/metric/put`, external/legacy MQTT) pass
untouched. The library's own publishers go through the crate-private `ReservedMessaging` seam —
in Rust the guard is compiler-enforced.

## Messages

A `Message` is a plain owned value type (`Clone`, `Send`, `Sync` — no shared mutable
state, so it can't race). Build it with `MessageBuilder`; the wire JSON shape matches
Java/Python for interoperation:

```rust
use ggcommons::messaging::message::MessageBuilder;
use serde_json::json;

let msg = MessageBuilder::new("ProcessData", "1.0")
    .from_config(&cfg)               // stamps the UNS identity (+ tags) from config
    .payload(json!({ "value": 42 }))
    .build();
svc.publish("plant/line-a/data", &msg).await?;
```

`uuid`, `correlation_id`, and the RFC3339 `timestamp` are stamped at construction.

### Wire format & parity

The envelope is `{header, identity, tags, body}`: `from_config` stamps the top-level **`identity`**
element (`{hier, path, component, instance}`, resolved from the `hierarchy`/`identity` config
blocks; `instance` defaults to `"main"` — override per message with `.instance("kep1")` or via
`gg.instance(id).message(...)`). The former `tags.thing` field is **removed** (hard cut); `tags`
carries business metadata only. Header keys are **snake_case** (`correlation_id`, `reply_to`) and
request/reply uses the `ggcommons/reply-` topic prefix — matching the Java/Python/TypeScript
`MessageHeader` exactly so the four libraries interoperate on the same topics (byte-identical
topics and structurally identical envelopes are pinned by the shared `uns-test-vectors/`).

A received payload that is **not an envelope** (no `header`/`tags`/`body`, or not even
JSON) is delivered as a **raw** message rather than dropped: check `Message::is_raw()`
and read `Message::get_raw()` (mirrors Java `getRaw()` / Python `Message.raw`). A raw
message serializes as `{ "raw": <value> }`.

### `receiveOwnMessages`

`GgCommonsBuilder::receive_own_messages(bool)` exists for parity (default `true`).
Setting it to `false` is currently a **no-op that logs a warning**: the
`aws-greengrass-component-sdk` exposes no IPC `ReceiveMode`, so own-message
suppression cannot be done natively, and no client-side scheme covers all message
shapes (raw messages carry no sender identity). See
[`sdk-receive-mode-feature-request.md`](./sdk-receive-mode-feature-request.md).

## Publish / subscribe

`subscribe` registers a [`MessageHandler`] (wrap a closure with `message_handler`)
and returns `()` — subscription handles are kept **internal** so a broker
subscription can never be orphaned. Stop a subscription only via `unsubscribe`,
which both aborts dispatch and UNSUBSCRIBEs at the broker.

Two independent settings per subscription:

- **`max_messages`** — bounds the client-side queue; the provider drops on overflow
  with a warning.
- **`max_concurrency`** — bounds simultaneous handler invocations (`1` = serial and
  ordered; `N` = up to N concurrent).

```rust
use ggcommons::messaging::message_handler;

svc.subscribe("events/+", message_handler(|topic, msg| async move {
    tracing::info!(%topic, name = %msg.header.name, "received");
}), 32, 4).await?;
```

## Request / reply

`request` returns a [`ReplyFuture`] (the Rust analog of Java `CompletableFuture` /
Python `Iou`). Every request arms a **framework-owned internal deadline** at send time
(`messaging.requestTimeoutSeconds`, default **30 s**, `0` disables): when it fires, the
pending entry is removed, the ephemeral reply topic is unsubscribed, and the future
resolves `Err(GgError::RequestTimeout { topic, secs })` — **even if the future is
never polled** (a supervisor task owns the reply subscription). Completion, deadline,
and `cancel_request` all UNSUBSCRIBE the reply topic — no leaks (fixing the Java H2
class of bug). Use `request_with_timeout(topic, msg, Some(duration))` for a per-call
override (`None` = the config default).

```rust
use std::time::Duration;

let reply = svc.request("svc/op", request_msg).await?.await?;          // config-default deadline
let fast  = svc.request_with_timeout("svc/op", msg2,
                Some(Duration::from_secs(5))).await?.await?;           // per-call override
```

The responder side correlates automatically:

```rust
svc.subscribe("svc/op", message_handler(move |_t, req| {
    let svc = svc.clone();
    async move {
        let reply = MessageBuilder::new("OpResult", "1.0").payload(json!({"ok": true})).build();
        let _ = svc.reply(&req, reply).await;
    }
}), 16, 1).await?;
```

Because correlation lives **above** the transport, it behaves identically over MQTT
and Greengrass IPC, and is fully testable against a local broker.

## MQTT messaging config

The MQTT transport requires a messaging-config JSON file (passed after
`--transport MQTT`, e.g. `--platform HOST --transport MQTT <messaging_config.json>`):

```json
{
  "messaging": {
    "local": {
      "host": "localhost",
      "port": 1883,
      "clientId": "my-component-local",
      "credentials": { "username": "u", "password": "p" }
    },
    "iotCore": {
      "endpoint": "xxxx-ats.iot.us-east-1.amazonaws.com",
      "port": 8883,
      "clientId": "my-component-iotcore",
      "credentials": {
        "certPath": "/certs/device.pem.crt",
        "keyPath": "/certs/private.pem.key",
        "caPath": "/certs/AmazonRootCA1.pem"
      }
    }
  }
}
```

- The `local` broker is required; `iotCore` is optional.
- IoT Core uses mutual TLS — `caPath` + `certPath` + `keyPath` are all required and a
  load failure is a hard error (never an unauthenticated fallback). A `caPath` on the
  local broker enables local TLS.
- Connections and subscriptions block until confirmed (CONNACK/SUBACK); the provider
  auto-reconnects and re-subscribes on disconnect.
- The `messaging` section also takes **`requestTimeoutSeconds`** (the request-deadline
  default, 30; `0` disables) and **`lwt`** (`{ topic, payload, qos }` — an MQTT
  Last-Will registered on the **local** connection at CONNECT, published verbatim by
  the broker on ungraceful disconnect; never retained, and the IPC provider no-ops it).
  The will is registered at CONNECT, not routed through `publish()`, so the
  reserved-class guard does not apply to it — broker ACLs govern wills.
