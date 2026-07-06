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

use crate::error::{EdgeCommonsError, Result};
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
    /// Live subscription handlers (local + IoT Core share one map — no test so far needs both
    /// destinations subscribed to the same filter), keyed by filter: inserted by
    /// `subscribe`/`subscribe_to_iot_core`, removed by `unsubscribe`/`unsubscribe_from_iot_core`.
    /// Backs [`Self::subscribed_topics`] / [`Self::simulate_message`] for tests exercising a
    /// `MessageHandler` (e.g. [`crate::uns::RepublishListener`], [`crate::commands::CommandInbox`]).
    pub handlers: Mutex<HashMap<String, Arc<dyn MessageHandler>>>,
    /// `(reply_to topic, reply message)` recorded by [`MessagingService::reply`] /
    /// `reply_to_iot_core` — the correlation id is stamped from the request first, mirroring
    /// [`crate::messaging::service::DefaultMessagingService`]'s real `reply()`. Backs
    /// [`Self::replies`] (the command-inbox tests' reply assertions).
    pub replied: Mutex<Vec<(String, Message)>>,
    /// When `true`, `reply`/`reply_to_iot_core` return an error instead of recording — drives
    /// the "a failing reply publish is swallowed" tests. Set via [`Self::set_fail_reply`].
    pub fail_reply: AtomicBool,
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

    /// The currently-subscribed filter set — grows on `subscribe`/`subscribe_to_iot_core`,
    /// shrinks on `unsubscribe`/`unsubscribe_from_iot_core`. Mirrors the Java
    /// `MockMessagingService.getSubscribedTopics()`.
    pub fn subscribed_topics(&self) -> std::collections::HashSet<String> {
        self.handlers.lock().unwrap().keys().cloned().collect()
    }

    /// Deliver `message` on `topic` to every subscription whose filter matches it (an
    /// exact-topic subscription matches itself, so plain-topic tests behave as before;
    /// wildcard subscriptions — e.g. the command inbox's `.../cmd/#` — receive concrete
    /// topics), via MQTT-style [`crate::messaging::topic_matches`]. A no-op when nothing
    /// matches. Mirrors the Java `MockMessagingService.simulateMessage`.
    pub async fn simulate_message(&self, topic: &str, message: Message) {
        let matched: Vec<Arc<dyn MessageHandler>> = {
            let handlers = self.handlers.lock().unwrap();
            handlers
                .iter()
                .filter(|(filter, _)| crate::messaging::topic_matches(filter, topic))
                .map(|(_, handler)| handler.clone())
                .collect()
        };
        for handler in matched {
            handler.handle(topic.to_string(), message.clone()).await;
        }
    }

    /// All `(reply_to topic, reply message)` pairs recorded via `reply`/`reply_to_iot_core`.
    pub fn replies(&self) -> Vec<(String, Message)> {
        self.replied.lock().unwrap().clone()
    }

    /// Make the next (and every subsequent) `reply`/`reply_to_iot_core` call fail instead of
    /// recording — simulates a broker/publish failure so tests can assert it is swallowed.
    pub fn set_fail_reply(&self, fail: bool) {
        self.fail_reply.store(fail, Ordering::SeqCst);
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
        handler: Arc<dyn MessageHandler>,
        _max_messages: usize,
        _max_concurrency: usize,
    ) -> Result<()> {
        self.subscribed.lock().unwrap().push(filter.to_string());
        self.handlers.lock().unwrap().insert(filter.to_string(), handler);
        Ok(())
    }

    async fn subscribe_to_iot_core(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        _qos: Qos,
        _max_messages: usize,
        _max_concurrency: usize,
    ) -> Result<()> {
        self.subscribed.lock().unwrap().push(filter.to_string());
        self.handlers.lock().unwrap().insert(filter.to_string(), handler);
        Ok(())
    }

    async fn unsubscribe(&self, filter: &str) -> Result<()> {
        self.handlers.lock().unwrap().remove(filter);
        Ok(())
    }

    async fn unsubscribe_from_iot_core(&self, filter: &str) -> Result<()> {
        self.handlers.lock().unwrap().remove(filter);
        Ok(())
    }

    async fn request(&self, _topic: &str, _msg: Message) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn request_from_iot_core(&self, _topic: &str, _msg: Message) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn request_with_timeout(
        &self,
        _topic: &str,
        _msg: Message,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn request_from_iot_core_with_timeout(
        &self,
        _topic: &str,
        _msg: Message,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging("request not supported by RecordingMessaging".into()))
    }

    async fn reply(&self, request: &Message, reply: Message) -> Result<()> {
        if self.fail_reply.load(Ordering::SeqCst) {
            return Err(EdgeCommonsError::Messaging("simulated reply failure".to_string()));
        }
        let topic = request.header.reply_to.clone().ok_or_else(|| {
            EdgeCommonsError::Messaging("cannot reply: request has no reply_to".to_string())
        })?;
        let mut reply = reply;
        reply.header.correlation_id = request.header.correlation_id.clone();
        self.replied.lock().unwrap().push((topic, reply));
        Ok(())
    }

    async fn reply_to_iot_core(&self, request: &Message, reply: Message) -> Result<()> {
        self.reply(request, reply).await
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
