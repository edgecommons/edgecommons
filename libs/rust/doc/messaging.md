# Messaging

Messaging is a two-layer design — a deliberate structural improvement over the Java
library, which duplicated request/reply inside each provider and let them drift.

- **Layer 1 — `MessagingProvider`** moves bytes on topics for a `Destination`
  (`Local` or `Northbound`) at a given `Qos`. Implementations: `MqttProvider`
  (the MQTT transport, dual broker) and `IpcProvider` (the IPC transport — Greengrass
  IPC, `greengrass` feature).
- **Layer 2 — `MessagingService`** is transport-agnostic and built **once** over any
  provider. It owns message (de)serialization, the callback dispatch model, and
  request/reply correlation. This is the API component authors use.

Obtain it from the runtime: `let svc = gg.messaging()?;` (returns `Err` only on the
IPC transport — i.e. `--platform GREENGRASS` — when the `greengrass` feature is disabled).

## Explicit local / northbound method pairs

Mirroring the Greengrass v2 API and the Java/Python `IMessagingService`, every
operation has an explicit local and northbound form (rather than a destination
argument):

| Local | Northbound |
|-------|------------|
| `publish` | `publish_northbound` (+ `qos`) |
| `publish_confirmed` | `publish_northbound_confirmed` |
| `publish_encoded_confirmed` | `publish_northbound_encoded_confirmed` |
| `publish_raw` | `publish_northbound_raw` (+ `qos`) |
| `subscribe` | `subscribe_northbound` (+ `qos`) |
| `unsubscribe` | `unsubscribe_northbound` |
| `request` / `request_with_timeout` | `request_northbound` / `request_northbound_with_timeout` |
| `reply` / `reply_confirmed` | `reply_northbound` / `reply_northbound_confirmed` |
| `cancel_request` | `cancel_request_northbound` |

The public Rust API uses `northbound` consistently: `Destination::Northbound` and
`*_northbound` method names. AWS IoT Core remains one possible northbound broker/Greengrass bridge,
not the public method family name.

## UNS topics & the reserved-class guard

Topics follow the Unified Namespace grammar `ecv1/{device}/{component}/{instance}/{class}[/channel]`
(see `docs/platform/DESIGN-uns.md`). Build them with the validating builder rather than by hand:

```rust
use edgecommons::uns::UnsClass;

let topic = gg.uns().topic_with_channel(UnsClass::App, "order/received")?;
// -> ecv1/gw-01/my-component/app/order/received
svc.publish(&topic, &msg).await?;

let inst = gg.instance("kep1")?;              // instance-scoped handle
let data = inst.uns().topic_with_channel(UnsClass::Data, "press12/temperature")?;
```

The four **reserved platform classes** — `state`, `metric`, `cfg`, `log` — are library-owned: every
publish-side method (`publish`, `publish_raw`, `request*`, `reply*`, and their northbound variants)
rejects a client-chosen reserved-class `ecv1/…` topic with `EdgeCommonsError::ReservedTopic`. Use
`gg.uns()` / the heartbeat/metric subsystems instead; `subscribe*` is never guarded. Non-`ecv1`
topics (the `edgecommons/reply-…` prefix, `cloudwatch/metric/put`, external/legacy MQTT) pass
untouched. The library's own publishers go through the crate-private `ReservedMessaging` seam —
in Rust the guard is compiler-enforced.

## Messages

A `Message` is a plain owned value type (`Clone`, `Send`, `Sync` — no shared mutable
state, so it can't race). Build it with `MessageBuilder`; the wire JSON shape matches
Java/Python for interoperation:

```rust
use edgecommons::messaging::message::MessageBuilder;
use serde_json::json;

let msg = MessageBuilder::new("ProcessData", "1.0")
    .from_config(&cfg)               // stamps the UNS identity (+ tags) from config
    .payload(json!({ "value": 42 }))
    .build();
svc.publish("plant/line-a/data", &msg).await?;
```

For small binary payloads, use `binary_payload` instead of trying to put bytes into
`serde_json::Value`:

```rust
let msg = MessageBuilder::new("Blob", "1.0")
    .binary_payload([0, 1, 2, 254, 255])?
    .build();
assert_eq!(msg.binary_body()?.unwrap(), vec![0, 1, 2, 254, 255]);
```

The wire body is the shared first-class binary marker
`{ "_edgecommonsBinary": { "encoding": "base64", "length": n, "data": "..." } }`.
Decoded binary bodies are limited to `MAX_BINARY_BODY_BYTES` (64 KiB). This is for
bounded control payloads, not frame/video streaming.

`uuid`, `correlation_id`, and the RFC3339 `timestamp` are stamped at construction.

### Wire format & parity

The envelope is `{header, identity, tags, body}`: `from_config` stamps the top-level **`identity`**
element (`{hier, path, component, instance}`, resolved from the `hierarchy`/`identity` config
blocks; `instance` is optional and omitted by default, giving a component-scoped topic — set one per
message with `.instance("kep1")` or via `gg.instance(id).message(...)` for instance scope). The former `tags.thing` field is **removed** (hard cut); `tags`
carries business metadata only. Header keys are **snake_case** (`correlation_id`, `reply_to`) and
request/reply uses the `edgecommons/reply-` topic prefix — matching the Java/Python/TypeScript
`MessageHeader` exactly so the four libraries interoperate on the same topics (byte-identical
topics and structurally identical envelopes are pinned by the shared `uns-test-vectors/`).

A received payload that is **not an envelope** (no `header`/`tags`/`body`, or not even
JSON) is delivered as a **raw** message rather than dropped: check `Message::is_raw()`
and read `Message::get_raw()` (mirrors Java `getRaw()` / Python `Message.raw`). A raw
message serializes as `{ "raw": <value> }`.

### `receiveOwnMessages`

`EdgeCommonsBuilder::receive_own_messages(bool)` exists for parity (default `true`).
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
use edgecommons::messaging::message_handler;

svc.subscribe("events/+", message_handler(|topic, msg| async move {
    tracing::info!(%topic, name = %msg.header.name, "received");
}), 32, 4).await?;
```

### Strict confirmed publication

Ordinary `publish` completes when the transport accepts the request into its bounded client path. Use
`publish_confirmed(topic, message, timeout)` when a durable outbox must distinguish enqueueing from
transport acknowledgement:

- standalone MQTT always uses QoS 1 and returns success only after the matching broker PUBACK;
- Greengrass returns success only after the IPC publish operation completes successfully;
- zero/expired timeout, disconnect before acknowledgement, and lost waiter state are errors with an
  ambiguous outcome; and
- a provider that cannot prove acknowledgement returns an unsupported error. It never delegates to
  ordinary `publish` and calls that confirmation.

Every MQTT publish goes through one bounded per-connection funnel. Ordinary QoS 0/1/2 publishes and
confirmed QoS 1 publishes all consume an ordered tracker marker. `rumqttc`'s
`Outgoing::Publish(packet_id)` assigns that marker, and only the matching incoming PUBACK settles its
confirmed waiter. Packet-id tombstones survive retransmission, so an ordinary concurrent publish or a
reconnect cannot shift a later PUBACK onto the wrong waiter.

`publish_encoded_confirmed` is the exact-byte durable-outbox seam. It first validates that the supplied
bytes decode as an EdgeCommons protobuf envelope, then applies the same reserved-topic and confirmation
semantics while sending the caller's original bytes without reserialization. Raw or malformed payloads
are rejected before provider I/O; use the raw publication APIs for non-envelope data.

```rust
use std::time::Duration;

svc.publish_confirmed("plant/camera/result", &msg, Duration::from_secs(5)).await?;
```

The app facade prepares stable envelopes for this path:

```rust
use serde_json::json;
use std::time::Duration;

let app = gg.instance("camera-1")?.app();
let prepared = app.prepare_correlated(
    "ImageCaptured",
    "image/captured",
    json!({ "captureId": "cap-1" }),
    &request,
)?;

// Persist prepared.topic(), prepared.message(), and prepared.encoded() before publish.
app.publish_prepared_confirmed(&prepared, Duration::from_secs(5)).await?;
```

`prepare_correlated` accepts either a received `&Message` or an explicit correlation-id string. The
correlation is stamped in the standard envelope header. `PreparedAppMessage::encoded()` is produced once;
confirmed prepared publication sends those stored bytes verbatim. Legacy `app.publish` and
`app.publish_via` delegate through preparation but retain their existing completion/error behavior.

## Request / reply

`request` returns a [`ReplyFuture`] (the Rust analog of Java `CompletableFuture` /
Python `Iou`). Every request arms a **framework-owned internal deadline** at send time
(`messaging.requestTimeoutSeconds`, default **30 s**, `0` disables): when it fires, the
pending entry is removed, the ephemeral reply topic is unsubscribed, and the future
resolves `Err(EdgeCommonsError::RequestTimeout { topic, secs })` — **even if the future is
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

Use `reply_confirmed(request, reply, timeout)` when the responder must retry until an actual QoS 1/IPC
acknowledgement. It retains the standard guarded `reply_to` behavior and copies the request correlation id.
The command inbox's deferred registry uses this strict path. See [commands.md](commands.md).

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
    "northbound": {
      "endpoint": "xxxx-ats.iot.us-east-1.amazonaws.com",
      "port": 8883,
      "clientId": "my-component-northbound",
      "credentials": {
        "certPath": "/certs/device.pem.crt",
        "keyPath": "/certs/private.pem.key",
        "caPath": "/certs/AmazonRootCA1.pem"
      }
    }
  }
}
```

- The `local` broker is required; `northbound` is optional.
- Local and northbound brokers are generic MQTT connections. `caPath` enables TLS; adding
  `certPath` + `keyPath` enables mutual TLS. Without `caPath`, the connection is plaintext.
- Connections and subscriptions block until confirmed (CONNACK/SUBACK); the provider
  auto-reconnects and re-subscribes on disconnect.
- The `messaging` section also takes **`requestTimeoutSeconds`** (the request-deadline
  default, 30; `0` disables). Each broker can carry **`qos`** defaults:
  `messaging.local.qos.publish`, `messaging.local.qos.subscribe`,
  `messaging.northbound.qos.publish`, and `messaging.northbound.qos.subscribe` accept `0`/`1`/`2`.
- Generic component messaging config does not define MQTT Last-Will. The first-party LWT use is
  `uns-bridge`'s private site-broker uplink Last-Will, derived internally from its resolved UNS
  state topic.
