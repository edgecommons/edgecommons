# Messaging

Messaging is a two-layer design — a deliberate structural improvement over the Java
library, which duplicated request/reply inside each provider and let them drift.

- **Layer 1 — `MessagingProvider`** moves bytes on topics for a `Destination`
  (`Local` or `IotCore`) at a given `Qos`. Implementations: `MqttProvider`
  (STANDALONE, dual broker) and `IpcProvider` (Greengrass IPC, `greengrass` feature).
- **Layer 2 — `MessagingService`** is transport-agnostic and built **once** over any
  provider. It owns message (de)serialization, the callback dispatch model, and
  request/reply correlation. This is the API component authors use.

Obtain it from the runtime: `let svc = gg.messaging()?;` (returns `Err` only in
GREENGRASS mode when the `greengrass` feature is disabled).

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
| `request` | `request_from_iot_core` |
| `reply` | `reply_to_iot_core` |
| `cancel_request` | `cancel_request_from_iot_core` |

## Messages

A `Message` is a plain owned value type (`Clone`, `Send`, `Sync` — no shared mutable
state, so it can't race). Build it with `MessageBuilder`; the wire JSON shape matches
Java/Python for interoperation:

```rust
use ggcommons::messaging::message::MessageBuilder;
use serde_json::json;

let msg = MessageBuilder::new("ProcessData", "1.0")
    .from_config(&cfg)               // copies thing name + tags
    .payload(json!({ "value": 42 }))
    .build();
svc.publish("plant/line-a/data", &msg).await?;
```

`uuid`, `correlation_id`, and the RFC3339 `timestamp` are stamped at construction.

### Wire format & parity

Header keys are **snake_case** (`correlation_id`, `reply_to`) and request/reply uses
the `ggcommons/reply-` topic prefix — both matching the Java/Python/TypeScript `MessageHeader`
exactly so the four libraries interoperate on the same topics.

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
Python `Iou`). Await it directly, or wrap it in `tokio::time::timeout` for a
deadline. **Completion, timeout, and `cancel_request` all UNSUBSCRIBE the ephemeral
reply topic** — no leaks (fixing the Java H2 class of bug).

```rust
use std::time::Duration;

let reply = tokio::time::timeout(
    Duration::from_secs(5),
    svc.request("svc/op", request_msg).await?,
).await??;
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

## STANDALONE messaging config

STANDALONE mode requires a messaging-config JSON file (passed after `-m STANDALONE`):

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
