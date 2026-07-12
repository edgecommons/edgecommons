//! # Messaging provider — Greengrass IPC
//!
//! **One-liner purpose**: A [`MessagingProvider`] that moves bytes over Greengrass
//! IPC (local pub/sub) and the IoT Core MQTT bridge, via the shared [`crate::ipc`]
//! runtime.
//!
//! ## Overview
//! [`IpcProvider`] is the GREENGRASS-mode counterpart of the standalone
//! [`super::mqtt::MqttProvider`]. It implements the same low-level transport
//! contract, so the transport-agnostic [`crate::messaging::service`] layer
//! (publish/subscribe, request/reply) works **unchanged** over Greengrass IPC.
//!
//! ## Semantics & Architecture
//! - All work is delegated to the process-global [`crate::ipc::IpcRuntime`] (one SDK
//!   per process). `subscribe` opens a bounded delivery channel (`max_messages`) and
//!   returns a [`Subscription`] whose drop guard closes the broker subscription.
//! - `unsubscribe(filter, dest)` is a no-op: the SDK ties broker-side teardown to
//!   dropping the subscription object, which the [`Subscription`] guard does on drop
//!   (when the service aborts the dispatcher). This matches the RAII cleanup model.
//! - Async (`tokio`); object-safe via `async_trait`.
//!
//! ## Status
//! Implemented and **validated on a live Greengrass core** (non-root): local pub/sub
//! with inbound delivery, request/reply, and the IoT Core bridge in both directions.
//!
//! ## Robustness
//! Inbound message delivery happens in SDK-internal callback threads (see
//! [`crate::ipc`]). Those callbacks are panic-contained so a panicking conversion or
//! delivery on one message can never poison an SDK thread and wedge the single
//! Greengrass IPC event loop — mirroring the Java `SubscriptionHandler` `try/catch`
//! fix. Late/duplicate replies are dropped via
//! [`crate::messaging::request_reply::try_deliver_reply`] rather than panicking,
//! mirroring the Java reply-future null-guard.
//!
//! ## Related Modules
//! - [`crate::ipc`], [`super::mqtt`], [`crate::messaging::service`].

#![cfg(feature = "greengrass")]

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{EdgeCommonsError, Result};
use crate::ipc;
use crate::messaging::{Destination, MessagingProvider, Qos, Subscription};

/// Greengrass IPC transport provider (delegates to the shared [`ipc::IpcRuntime`]).
pub struct IpcProvider {
    _private: (),
}

/// Closes a Greengrass IPC subscription on drop (RAII), by id, via the runtime.
struct IpcSubGuard {
    id: u64,
}

impl Drop for IpcSubGuard {
    fn drop(&mut self) {
        ipc::global().unsubscribe(self.id);
    }
}

impl IpcProvider {
    /// Connect to the Greengrass nucleus and return a ready provider.
    ///
    /// # Errors
    /// | Error Variant | Condition | Recovery |
    /// |---------------|-----------|----------|
    /// | `EdgeCommonsError::Ipc` | The nucleus is unreachable or the required env vars are unset | Run under a Greengrass core; check `SVCUID` / socket path |
    pub async fn connect() -> Result<IpcProvider> {
        ipc::global().connect().await?;
        Ok(IpcProvider { _private: () })
    }
}

#[async_trait]
impl MessagingProvider for IpcProvider {
    async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
    ) -> Result<()> {
        ipc::global().publish(topic, payload, dest, qos).await
    }

    async fn publish_confirmed(
        &self,
        topic: &str,
        payload: Vec<u8>,
        dest: Destination,
        qos: Qos,
        timeout: Duration,
    ) -> Result<()> {
        if qos != Qos::AtLeastOnce {
            return Err(EdgeCommonsError::Messaging(
                "confirmed Greengrass publish requires QoS 1".to_string(),
            ));
        }
        if timeout.is_zero() {
            return Err(EdgeCommonsError::Messaging(
                "confirmed Greengrass publish requires a positive timeout".to_string(),
            ));
        }
        match tokio::time::timeout(
            timeout,
            ipc::global().publish(topic, payload, dest, Qos::AtLeastOnce),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(EdgeCommonsError::Messaging(format!(
                "confirmed Greengrass publish to '{topic}' timed out after {}s; outcome is ambiguous",
                timeout.as_secs_f64()
            ))),
        }
    }

    async fn subscribe(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
    ) -> Result<Subscription> {
        let (tx, rx) = tokio::sync::mpsc::channel(max_messages.max(1));
        let id = ipc::global().subscribe(filter, dest, qos, tx).await?;
        Ok(Subscription::new(rx, Box::new(IpcSubGuard { id })))
    }

    async fn subscribe_acknowledged(
        &self,
        filter: &str,
        dest: Destination,
        qos: Qos,
        max_messages: usize,
        timeout: Duration,
    ) -> Result<Subscription> {
        if timeout.is_zero() {
            return Err(EdgeCommonsError::Messaging(
                "acknowledged Greengrass subscribe requires a positive timeout".to_string(),
            ));
        }
        match tokio::time::timeout(timeout, self.subscribe(filter, dest, qos, max_messages)).await {
            Ok(result) => result,
            Err(_) => Err(EdgeCommonsError::Messaging(format!(
                "Greengrass subscription operation for '{filter}' timed out after {}s",
                timeout.as_secs_f64()
            ))),
        }
    }

    async fn unsubscribe(&self, _filter: &str, _dest: Destination) -> Result<()> {
        // Broker-side teardown happens when the Subscription's guard drops (RAII);
        // there is no filter-keyed unsubscribe in the SDK.
        Ok(())
    }

    fn connected(&self) -> bool {
        // The IPC client is built (and `connect()` succeeded) before this provider exists, so the
        // Nucleus-side connection is established. Greengrass exposes no liveness probe on the IPC
        // socket, so "the client is built" is the connected signal (parity with Java IPC readiness).
        true
    }
}
