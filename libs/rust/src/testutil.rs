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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Notify;

use crate::error::{EdgeCommonsError, Result};
use crate::messaging::Qos;
use crate::messaging::message::Message;
use crate::messaging::service::{MessageHandler, MessagingService, ReplyFuture, ReservedMessaging};
use crate::metrics::{Metric, MetricService};

/// A [`MessagingService`] (+ crate-private [`ReservedMessaging`] seam) that records
/// published messages for assertions — test fakes implement both traits (§4.2).
#[derive(Default)]
pub(crate) struct RecordingMessaging {
    /// `(topic, message)` published to the local broker.
    pub published: Mutex<Vec<(String, Message)>>,
    /// `(topic, message)` published to IoT Core.
    pub iot_published: Mutex<Vec<(String, Message)>>,
    /// Exact bytes supplied to confirmed local publication.
    pub confirmed_local: Mutex<Vec<(String, Vec<u8>)>>,
    /// Exact bytes supplied to confirmed northbound publication.
    pub confirmed_iot: Mutex<Vec<(String, Vec<u8>)>>,
    /// `(topic, message)` published locally through the privileged seam.
    pub reserved_published: Mutex<Vec<(String, Message)>>,
    /// `(topic, message)` published to IoT Core through the privileged seam.
    pub reserved_iot_published: Mutex<Vec<(String, Message)>>,
    /// Topics subscribed to locally.
    pub subscribed: Mutex<Vec<String>>,
    /// Live subscription handlers (local + IoT Core share one map — no test so far needs both
    /// destinations subscribed to the same filter), keyed by filter: inserted by
    /// `subscribe`/`subscribe_northbound`, removed by `unsubscribe`/`unsubscribe_northbound`.
    /// Backs [`Self::subscribed_topics`] / [`Self::simulate_message`] for tests exercising a
    /// `MessageHandler` (e.g. [`crate::uns::RepublishListener`], [`crate::commands::CommandInbox`]).
    pub handlers: Mutex<HashMap<String, Arc<dyn MessageHandler>>>,
    /// `(reply_to topic, reply message)` recorded by [`MessagingService::reply`] /
    /// `reply_northbound` — the correlation id is stamped from the request first, mirroring
    /// [`crate::messaging::service::DefaultMessagingService`]'s real `reply()`. Backs
    /// [`Self::replies`] (the command-inbox tests' reply assertions).
    pub replied: Mutex<Vec<(String, Message)>>,
    /// When `true`, `reply`/`reply_northbound` return an error instead of recording — drives
    /// the "a failing reply publish is swallowed" tests. Set via [`Self::set_fail_reply`].
    pub fail_reply: AtomicBool,
    /// Number of upcoming confirmed publish/reply attempts which fail before recording.
    pub confirmed_failures_remaining: AtomicUsize,
    /// Monotonic timestamps of each publish (any path), for timing tests.
    pub publish_times: Mutex<Vec<Instant>>,
    /// The value [`MessagingService::connected`] returns (default `false`); set via
    /// [`RecordingMessaging::set_connected`] to drive readiness tests.
    pub connected: AtomicBool,
    /// Command-start acknowledgement controls for lifecycle tests.
    pub block_subscribe_ack: AtomicBool,
    pub subscribe_ack_entered: Notify,
    pub subscribe_ack_release: Notify,
    pub subscribe_ack_failure: Mutex<Option<String>>,
    pub deliver_during_subscribe_ack: Mutex<Vec<(String, Message)>>,
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

    /// Exact bytes sent through confirmed local publication.
    pub fn confirmed_local(&self) -> Vec<(String, Vec<u8>)> {
        self.confirmed_local.lock().unwrap().clone()
    }

    /// Exact bytes sent through confirmed northbound publication.
    pub fn confirmed_iot(&self) -> Vec<(String, Vec<u8>)> {
        self.confirmed_iot.lock().unwrap().clone()
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

    /// The currently-subscribed filter set — grows on `subscribe`/`subscribe_northbound`,
    /// shrinks on `unsubscribe`/`unsubscribe_northbound`. Mirrors the Java
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

    /// All `(reply_to topic, reply message)` pairs recorded via `reply`/`reply_northbound`.
    pub fn replies(&self) -> Vec<(String, Message)> {
        self.replied.lock().unwrap().clone()
    }

    /// Make the next (and every subsequent) `reply`/`reply_northbound` call fail instead of
    /// recording — simulates a broker/publish failure so tests can assert it is swallowed.
    pub fn set_fail_reply(&self, fail: bool) {
        self.fail_reply.store(fail, Ordering::SeqCst);
    }

    /// Fail the next `attempts` confirmed publish/reply calls.
    pub fn fail_next_confirmed(&self, attempts: usize) {
        self.confirmed_failures_remaining
            .store(attempts, Ordering::SeqCst);
    }

    fn consume_confirmed_failure(&self) -> bool {
        self.confirmed_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                (remaining > 0).then(|| remaining - 1)
            })
            .is_ok()
    }
}

#[async_trait]
impl ReservedMessaging for RecordingMessaging {
    async fn publish_reserved(&self, topic: &str, msg: &Message) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.reserved_published
            .lock()
            .unwrap()
            .push((topic.to_string(), msg.clone()));
        Ok(())
    }

    async fn publish_reserved_northbound(
        &self,
        topic: &str,
        msg: &Message,
        _qos: Qos,
    ) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.reserved_iot_published
            .lock()
            .unwrap()
            .push((topic.to_string(), msg.clone()));
        Ok(())
    }

    fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl MessagingService for RecordingMessaging {
    async fn publish(&self, topic: &str, msg: &Message) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.published
            .lock()
            .unwrap()
            .push((topic.to_string(), msg.clone()));
        Ok(())
    }

    async fn publish_northbound(&self, topic: &str, msg: &Message, _qos: Qos) -> Result<()> {
        self.publish_times.lock().unwrap().push(Instant::now());
        self.iot_published
            .lock()
            .unwrap()
            .push((topic.to_string(), msg.clone()));
        Ok(())
    }

    async fn publish_confirmed(
        &self,
        topic: &str,
        msg: &Message,
        timeout: std::time::Duration,
    ) -> Result<()> {
        self.publish_encoded_confirmed(topic, &msg.to_vec()?, timeout)
            .await
    }

    async fn publish_northbound_confirmed(
        &self,
        topic: &str,
        msg: &Message,
        timeout: std::time::Duration,
    ) -> Result<()> {
        self.publish_northbound_encoded_confirmed(topic, &msg.to_vec()?, timeout)
            .await
    }

    async fn publish_encoded_confirmed(
        &self,
        topic: &str,
        payload: &[u8],
        _timeout: std::time::Duration,
    ) -> Result<()> {
        if self.consume_confirmed_failure() {
            return Err(EdgeCommonsError::Messaging(
                "simulated confirmed publish failure".to_string(),
            ));
        }
        let message = Message::from_slice(payload)?;
        self.publish_times.lock().unwrap().push(Instant::now());
        self.confirmed_local
            .lock()
            .unwrap()
            .push((topic.to_string(), payload.to_vec()));
        self.published
            .lock()
            .unwrap()
            .push((topic.to_string(), message));
        Ok(())
    }

    async fn publish_northbound_encoded_confirmed(
        &self,
        topic: &str,
        payload: &[u8],
        _timeout: std::time::Duration,
    ) -> Result<()> {
        if self.consume_confirmed_failure() {
            return Err(EdgeCommonsError::Messaging(
                "simulated confirmed publish failure".to_string(),
            ));
        }
        let message = Message::from_slice(payload)?;
        self.publish_times.lock().unwrap().push(Instant::now());
        self.confirmed_iot
            .lock()
            .unwrap()
            .push((topic.to_string(), payload.to_vec()));
        self.iot_published
            .lock()
            .unwrap()
            .push((topic.to_string(), message));
        Ok(())
    }

    async fn publish_raw(&self, topic: &str, payload: &Value) -> Result<()> {
        // Record as a raw message so tests can read it via `get_raw()`.
        let msg = Message::raw(payload.clone());
        self.publish_times.lock().unwrap().push(Instant::now());
        self.published
            .lock()
            .unwrap()
            .push((topic.to_string(), msg));
        Ok(())
    }

    async fn publish_northbound_raw(&self, topic: &str, payload: &Value, _qos: Qos) -> Result<()> {
        let msg = Message::raw(payload.clone());
        self.publish_times.lock().unwrap().push(Instant::now());
        self.iot_published
            .lock()
            .unwrap()
            .push((topic.to_string(), msg));
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
        self.handlers
            .lock()
            .unwrap()
            .insert(filter.to_string(), handler);
        Ok(())
    }

    async fn subscribe_acknowledged(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        max_messages: usize,
        max_concurrency: usize,
        _timeout: Duration,
    ) -> Result<()> {
        self.subscribe(filter, handler.clone(), max_messages, max_concurrency)
            .await?;
        self.subscribe_ack_entered.notify_one();
        let deliveries = std::mem::take(&mut *self.deliver_during_subscribe_ack.lock().unwrap());
        for (topic, message) in deliveries {
            handler.handle(topic, message).await;
        }
        // D-U28: the command inbox now issues TWO acknowledged subscribes (instance- and
        // component-scope filters). The block is a one-shot latch so it models a single slow
        // ack window — the first subscribe blocks, the second proceeds — rather than deadlocking
        // the second subscribe on a release the test only sends once.
        if self.block_subscribe_ack.swap(false, Ordering::SeqCst) {
            self.subscribe_ack_release.notified().await;
        }
        if let Some(error) = self.subscribe_ack_failure.lock().unwrap().clone() {
            self.handlers.lock().unwrap().remove(filter);
            return Err(EdgeCommonsError::Messaging(error));
        }
        Ok(())
    }

    async fn subscribe_northbound(
        &self,
        filter: &str,
        handler: Arc<dyn MessageHandler>,
        _qos: Qos,
        _max_messages: usize,
        _max_concurrency: usize,
    ) -> Result<()> {
        self.subscribed.lock().unwrap().push(filter.to_string());
        self.handlers
            .lock()
            .unwrap()
            .insert(filter.to_string(), handler);
        Ok(())
    }

    async fn unsubscribe(&self, filter: &str) -> Result<()> {
        self.handlers.lock().unwrap().remove(filter);
        Ok(())
    }

    async fn unsubscribe_northbound(&self, filter: &str) -> Result<()> {
        self.handlers.lock().unwrap().remove(filter);
        Ok(())
    }

    async fn request(&self, _topic: &str, _msg: Message) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging(
            "request not supported by RecordingMessaging".into(),
        ))
    }

    async fn request_northbound(&self, _topic: &str, _msg: Message) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging(
            "request not supported by RecordingMessaging".into(),
        ))
    }

    async fn request_with_timeout(
        &self,
        _topic: &str,
        _msg: Message,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging(
            "request not supported by RecordingMessaging".into(),
        ))
    }

    async fn request_northbound_with_timeout(
        &self,
        _topic: &str,
        _msg: Message,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ReplyFuture> {
        Err(EdgeCommonsError::Messaging(
            "request not supported by RecordingMessaging".into(),
        ))
    }

    async fn reply(&self, request: &Message, reply: Message) -> Result<()> {
        if self.fail_reply.load(Ordering::SeqCst) {
            return Err(EdgeCommonsError::Messaging(
                "simulated reply failure".to_string(),
            ));
        }
        let topic = request.header.reply_to.clone().ok_or_else(|| {
            EdgeCommonsError::Messaging("cannot reply: request has no reply_to".to_string())
        })?;
        let mut reply = reply;
        reply.header.correlation_id = request.header.correlation_id.clone();
        self.replied.lock().unwrap().push((topic, reply));
        Ok(())
    }

    async fn reply_northbound(&self, request: &Message, reply: Message) -> Result<()> {
        self.reply(request, reply).await
    }

    async fn reply_confirmed(
        &self,
        request: &Message,
        reply: Message,
        _timeout: std::time::Duration,
    ) -> Result<()> {
        if self.consume_confirmed_failure() {
            return Err(EdgeCommonsError::Messaging(
                "simulated confirmed reply failure".to_string(),
            ));
        }
        self.reply(request, reply).await
    }

    async fn reply_northbound_confirmed(
        &self,
        request: &Message,
        reply: Message,
        timeout: std::time::Duration,
    ) -> Result<()> {
        self.reply_confirmed(request, reply, timeout).await
    }

    fn cancel_request(&self, _reply_future: ReplyFuture) {}

    fn cancel_request_northbound(&self, _reply_future: ReplyFuture) {}

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
        self.defined
            .lock()
            .unwrap()
            .push(metric.get_name().to_string());
    }

    fn is_metric_defined(&self, name: &str) -> bool {
        self.defined.lock().unwrap().iter().any(|n| n == name)
    }

    async fn emit_metric(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        self.emitted
            .lock()
            .unwrap()
            .push((name.to_string(), values));
        Ok(())
    }

    async fn emit_metric_now(&self, name: &str, values: HashMap<String, f64>) -> Result<()> {
        self.emitted
            .lock()
            .unwrap()
            .push((name.to_string(), values));
        Ok(())
    }

    async fn flush_metrics(&self) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) {}
}
