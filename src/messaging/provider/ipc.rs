//! # Messaging provider — Greengrass IPC (Phase 2)
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
//! Phase 2, **compile-only** — builds against the SDK on Linux; not yet validated
//! against a live Greengrass core.
//!
//! ## Related Modules
//! - [`crate::ipc`], [`super::mqtt`], [`crate::messaging::service`].

#![cfg(feature = "greengrass")]

use async_trait::async_trait;

use crate::error::Result;
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
    /// | `GgError::Ipc` | The nucleus is unreachable or the required env vars are unset | Run under a Greengrass core; check `SVCUID` / socket path |
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

    async fn unsubscribe(&self, _filter: &str, _dest: Destination) -> Result<()> {
        // Broker-side teardown happens when the Subscription's guard drops (RAII);
        // there is no filter-keyed unsubscribe in the SDK.
        Ok(())
    }
}
