//! # Test utilities (test-only)
//!
//! **One-liner purpose**: Shared fakes for unit tests, primarily a recording
//! [`crate::messaging::MessagingService`].
//!
//! ## Overview
//! [`RecordingMessaging`] implements the messaging service contract by recording
//! published messages (local and IoT Core) so tests can assert on them, without a
//! broker. The request/reply methods are intentionally unsupported here: request
//! correlation is covered by [`crate::messaging::service`]'s own tests against a fake
//! provider; subsystems that only publish (heartbeat, metrics) use this fake.
//!
//! Compiled only under `#[cfg(test)]`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{GgError, Result};
use crate::messaging::message::Message;
use crate::messaging::service::{MessageHandler, MessagingService, ReplyFuture, ReservedMessaging};
use crate::messaging::Qos;
use crate::metrics::{Metric, MetricService};

/// A [`MessagingService`] (+ crate-private [`ReservedMessaging`] seam) that records
/// published messages for assertions — test fakes implement both traits (§4.2).
#[derive(Default)]
pub(crate) struct RecordingMessaging {
    /// `(topic, message)` published to the local broker.
    pub published: Mutex<Vec<(String, Message)>>,
    /// `(topic, message)` published to IoT Core.
    pub iot_published: Mutex<Vec<(String, Message)>>,
    /// `(topic, message)` published locally through the privileged seam.
    pub reserved_published: Mutex<Vec<(String, Message)>>,
    /// `(topic, message)` published to IoT Core through the privileged seam.
    pub reserved_iot_published: Mutex<Vec<(String, Message)>>,
    /// Topics subscribed to locally.
    pub subscribed: Mutex<Vec<String>>,
    /// Monotonic timestamps of each publish (any path), for timing tests.
    pub publish_times: Mutex<Vec<Instant>>,
    /// The value [`MessagingService::connected`] returns (default `false`); set via
    /// [`RecordingMessaging::set_connected`] to drive readiness tests.
    pub connected: AtomicBool,
}

impl RecordingMessaging {
    /// A new, empty recorder wrapped in an `Arc` for injection.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// All `(topic, message)` pairs published to the local broker so far.
    pub fn local(&self) -> Vec<(String, Message)> {
        self.published.lock().unwrap().clone()
    }

    /// All `(topic, message)` pairs published to IoT Core so far.
    #[allow(dead_code)] // parity accessor with local(); kept for future tests
    pub fn iot(&self) -> Vec<(String, Message)> {
        self.iot_published.lock().unwrap().clone()
    }

    /// All `(topic, message)` pairs published locally through the privileged seam.
    pub fn reserved_local(&self) -> Vec<(String, Message)> {
        self.reserved_published.lock().unwrap().clone()
    }

    /// All `(topic, message)` pairs published to IoT Core through the privileged seam.
    pub fn reserved_iot(&self) -> Vec<(String, Message)> {
        self.reserved_iot_published.lock().unwrap().clone()
    }

    /// Monotonic timestamps of each publish, in order.
    pub fn times(&self) -> Vec<Instant> {
        self.publish_times.lock().unwrap().clone()
    }

    /// Set the value reported by [`MessagingService::connected`] (drives `/readyz` tests).
    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
    }
}

#[async_trait]
impl ReservedMessaging for RecordingMessaging {
    async fn publish_reserved(&self, topic: &str, msg: &Message) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.reserved_published.lock().unwrap().push((topic.to_string(), msg.clone()));
        Ok(())
    }

    async fn publish_reserved_to_iot_core(
        &self,
        topic: &str,
        msg: &Message,
        _qos: Qos,
    ) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.reserved_iot_published.lock().unwrap().push((topic.to_string(), msg.clone()));
        Ok(())
    }
}

#[async_trait]
impl MessagingService for RecordingMessaging {
    async fn publish(&self, topic: &str, msg: &Message) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.published.lock().unwrap().push((topic.to_string(), msg.clone()));
        Ok(())
    }

    async fn publish_to_iot_core(&self, topic: &str, msg: &Message, _qos: Qos) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.iot_published.lock().unwrap().push((topic.to_string(), msg.clone()));
        Ok(())
    }

    async fn publish_raw(&self, topic: &str, payload: &Value) -> Result<()> {
        // Record as a raw message so tests can read it via `get_raw()`.
        let msg = Message::raw(payload.clone());
        self.publish_times.lock().unwrap().push(Instant::now());
        self.published.lock().unwrap().push((topic.to_string(), msg));
        Ok(())
    }

    async fn publish_to_iot_core_raw(&self, topic: &str, payload: &Value, _qos: Qos) -> Result<()> {
        let msg = Message::raw(payload.clone());
        self.publish_times.lock().unwrap().push(Instant::now());
        self.iot_published.lock().unwrap().push((topic.to_string(), msg));
        Ok(())
    }

    async fn subscribe(
        &self,
        filter: &str,
        _handler: Arc<dyn MessageHandler>,
        _max_messages: usize,
        _max_concurrency: usize,
    ) -> Result<()> {
        self.subscribed.lock().unwrap().push(filter.to_string());
        Ok(())
    }

    async fn subscribe_to_iot_core(
        &self,
        filter: &str,
        _handler: Arc<dyn MessageHandler>,
        _qos: Qos,
        _max_messages: usize,
        _max_concurrency: usize,
    ) -> Result<()> {
        self.subscribed.lock().unwrap().push(filter.to_string());
        Ok(())
    }

    async fn unsubscribe(&self, _filter: &str) -> Result<()> {
        Ok(())
    }

    async fn unsubscribe_from_iot_core(&self, _filter: &str) -> Result<()> {
        Ok(())
    }

    async fn request(&self, _topic: &str, _msg: Message) -> Result<ReplyFuture> {
        Err(GgError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn request_from_iot_core(&self, _topic: &str, _msg: Message) -> Result<ReplyFuture> {
        Err(GgError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn request_with_timeout(
        &self,
        _topic: &str,
        _msg: Message,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ReplyFuture> {
        Err(GgError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn request_from_iot_core_with_timeout(
        &self,
        _topic: &str,
        _msg: Message,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ReplyFuture> {
        Err(GgError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn reply(&self, _request: &Message, _reply: Message) -> Result<()> {
        Ok(())
    }

    async fn reply_to_iot_core(&self, _request: &Message, _reply: Message) -> Result<()> {
        Ok(())
    }

    fn cancel_request(&self, _reply_future: ReplyFuture) {}

    fn cancel_request_from_iot_core(&self, _reply_future: ReplyFuture) {}

    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

/// A [`MetricService`] that records defined metrics and emitted measure maps.
#[derive(Default)]
pub(crate) struct RecordingMetrics {
    pub defined: Mutex<Vec<String>>,
    /// `(metric_name, values)` for each emit (buffered or immediate).
    pub emitted: Mutex<Vec<(String, HashMap<String, f64>)>>,
}

impl RecordingMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn emissions(&self) -> Vec<(String, HashMap<String, f64>)> {
        self.emitted.lock().unwrap().clone()
    }
}

#[async_trait]
impl MetricService for RecordingMetrics {
    fn define_metric(&self, metric: Metric) {
        self.defined.lock().unwrap().push(metric.get_name().to_string());
    }

    fn is_metric_defined(&self, name: &str) -> bool {
        self.defined.lock().unwrap().iter().any(|n| n == name)
    }

    async fn emit_metric(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        self.emitted.lock().unwrap().push((name.to_string(), values));
        Ok(())
    }

    async fn emit_metric_now(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        self.emitted.lock().unwrap().push((name.to_string(), values));
        Ok(())
    }

    async fn flush_metrics(&self) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) {}
}
