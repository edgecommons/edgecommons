//! Messaging subsystem (Phase 1).
//!
//! Two-layer design:
//! 1. `MessagingProvider` — transport primitives (publish / subscribe /
//!    unsubscribe). Implementations: dual-broker MQTT (`standalone`) and
//!    Greengrass IPC (`greengrass`, Phase 2).
//! 2. `MessagingService` — transport-agnostic, built **once** over any provider;
//!    owns `Message` (de)serialization and request/reply correlation (with
//!    timeout). Because correlation lives above the transport, it is identical
//!    over MQTT and IPC and fully testable over a local broker.
//!
//! This module currently defines the shared value types; the provider and
//! service implementations land in Phase 1.

/// Which broker a message targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// Local broker (standalone) or local IPC pub/sub (Greengrass).
    Local,
    /// AWS IoT Core.
    IotCore,
}

/// MQTT-style quality of service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    AtMostOnce,
    AtLeastOnce,
}
