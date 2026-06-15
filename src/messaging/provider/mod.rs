//! # Messaging — providers
//!
//! **One-liner purpose**: Concrete [`crate::messaging::MessagingProvider`]
//! transport implementations.
//!
//! ## Overview
//! - [`mqtt`] (feature `standalone`): dual-broker MQTT via `rumqttc`.
//! - Greengrass IPC provider: Phase 2.
//!
//! ## Related Modules
//! - [`crate::messaging`] — defines the provider trait and value types.

#[cfg(feature = "standalone")]
pub mod mqtt;
