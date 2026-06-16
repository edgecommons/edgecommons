//! # Messaging — providers
//!
//! **One-liner purpose**: Concrete [`crate::messaging::MessagingProvider`]
//! transport implementations.
//!
//! ## Overview
//! - [`mqtt`] (feature `standalone`): dual-broker MQTT via `rumqttc`.
//! - `ipc` (feature `greengrass`): Greengrass IPC via the shared SDK runtime.
//!
//! ## Related Modules
//! - [`crate::messaging`] — defines the provider trait and value types.

#[cfg(feature = "greengrass")]
pub mod ipc;
#[cfg(feature = "standalone")]
pub mod mqtt;
