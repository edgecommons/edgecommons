# Feature request (upstream): expose `ReceiveMode` on `SubscribeToTopic`

**Target project:** `aws-greengrass-component-sdk` (the Rust Greengrass component SDK,
crate version `1.0.4` as used here; lib name `gg_sdk`).

**Status:** draft, ready to file upstream. Tracks the `receiveOwnMessages` parity gap
in the GGCommons Rust port (see `B2` in the parity audit / `GGCOMMONS_RUST_PORT.md`).

## Summary

The Greengrass IPC `SubscribeToTopic` operation accepts an optional **`receiveMode`**
that controls whether a component receives messages it itself published on a topic it
is also subscribed to:

- `RECEIVE_ALL_MESSAGES` (default) — receive everything, including your own.
- `RECEIVE_MESSAGES_FROM_OTHERS` — the nucleus filters out the subscriber's own
  messages, by **connection/component identity**, before delivery.

The Rust SDK's safe API (`Sdk::subscribe_to_topic`) and the underlying C binding
(`ggipc_subscribe_to_topic`) expose **no** way to set `receiveMode`. There is no
`ReceiveMode` type anywhere in the crate. This request asks for that option to be
surfaced.

## Motivation

The AWS Greengrass **Java** and **Python** component libraries both expose a
`receiveOwnMessages` flag and implement it via this native `ReceiveMode`:

- Java: `GreengrassMessagingProvider` sets
  `receiveMode = receiveOwnMessages ? RECEIVE_ALL_MESSAGES : RECEIVE_MESSAGES_FROM_OTHERS`.
- Python: equivalent, via the `awsiot` IPC SDK.

The GGCommons **Rust** port aims for behavioral parity with those libraries. Without
`ReceiveMode` in the SDK, `receiveOwnMessages = false` cannot be honored:

- It is **broker-side and identity-based** — the only correct way to suppress a
  component's own messages. The nucleus knows the sender; the subscriber does not.
- A **client-side** workaround is unsatisfactory:
  - Tracking recently-published message ids needs an unbounded cache for correctness
    (a bounded cache drops the guarantee under high publish rates) — a memory vs.
    correctness trade-off with no good answer, plus lock contention on the hot path.
  - Tagging outgoing messages with a per-process source id does not cover **raw**
    (non-envelope) messages, which carry no header/tags to identify the sender.

As a result the Rust port currently treats `receiveOwnMessages = false` as a
documented no-op (it logs a warning and behaves as `true`).

## Proposed API

Add a `ReceiveMode` enum and an opt-in subscribe variant (keeping the existing
`subscribe_to_topic` as the `RECEIVE_ALL_MESSAGES` default for backwards
compatibility):

```rust
/// Mirrors the Greengrass IPC SubscribeToTopic `receiveMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveMode {
    /// Receive all messages, including the subscriber's own (the IPC default).
    ReceiveAllMessages,
    /// Receive only messages published by *other* components (filtered by the
    /// nucleus on sender identity).
    ReceiveMessagesFromOthers,
}

impl Sdk {
    /// Subscribe with an explicit receive mode.
    pub fn subscribe_to_topic_with_mode<'a, F: Fn(&str, SubscribeToTopicPayload)>(
        &self,
        topic: &str,
        receive_mode: ReceiveMode,
        callback: &'a F,
    ) -> Result<Subscription<'a, F>>;
}
```

This requires threading a `receiveMode` argument through the C binding
`ggipc_subscribe_to_topic` to the underlying CBOR `SubscribeToTopicRequest`.

## References

- Greengrass IPC publish/subscribe — `SubscribeToTopic` / `receiveMode`:
  https://docs.aws.amazon.com/greengrass/v2/developerguide/ipc-publish-subscribe.html
- Java SDK model: `software.amazon.awssdk.aws.greengrass.model.ReceiveMode`.

## Workaround until available

GGCommons Rust keeps the `receive_own_messages(bool)` builder flag for API parity and
forward-compatibility; `false` logs a warning and is a no-op. When the SDK adds
`ReceiveMode`, wire the flag through `subscribe_to_topic_with_mode` for the local
(pub/sub) destination and remove the warning.
